//! Redaction (architecture §11.2, execution plan M8.5–M8.7). Layer L4.
//!
//! Content is marked for redaction with Redact annotations (§12.5.6.23); `pdfcraft-annot`
//! creates them. [`apply`] then removes everything under the marks, for good:
//! - text glyphs (the rest of each line keeps its position), inline images, and paths that the
//!   marks cover; images and vectors partly under a mark lose the covered part;
//! - content inside form XObjects (rewritten as new objects, so pages sharing them keep theirs);
//! - comments, links and form fields whose rectangle overlaps a mark;
//! - the marks themselves, replaced by boxes in their fill colour (with their overlay text)
//!   drawn into the page.
//!
//! A verification pass then re-reads every redacted page; if any glyph or inline image is still
//! under a region the whole operation fails (callers keep the previous document: fail-closed).
//! Finally, document metadata (`/Info` entries, XMP) that repeats the removed text is removed.
//! Places it leaves as they are (bookmark titles, attachments, remaining comments, other XMP)
//! are only counted when they still hold that text ([`Report::still_holding`]). Last, a shared
//! JBIG2 symbol dictionary (`/JBIG2Globals`) that anything gone by then used leaves the file too:
//! what still names it is stored again without it, or the redaction is refused.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

pub mod codes;
mod elsewhere;
mod globals;
mod identification;
mod image;
mod interp;
mod metadata;
mod overlay;
mod pattern_guard;
pub mod patterns;
pub mod sanitize;
mod tags;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_verify;
pub(crate) mod type3;
pub(crate) mod type3_tables;
pub mod user_regex;
pub mod verify;

pub use elsewhere::Elsewhere;
pub use identification::{Claim, Identification};
use interp::{Mode, Scope, process};
pub use pattern_guard::PatternProblem;
pub use verify::{Proof, ProofOptions, Snapshot};

pub type Rgb = [f64; 3];

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RedactError {
    #[error("there are no redaction marks to apply")]
    NothingToApply,
    #[error("page {0} has content that can't be read, so it can't be redacted safely")]
    Unreadable(usize),
    /// Redact annotations with no readable area (neither usable `/QuadPoints` nor a usable `/Rect`).
    /// Skipping them would leave the text under them in the file while the others report success.
    #[error(
        "{0} redaction mark(s) have no readable area (neither usable QuadPoints nor a usable Rect), so the text under them can't be known; nothing was redacted. Remove those marks or mark the text again, then apply again"
    )]
    UnreadableMark(usize),
    #[error("redaction could not be verified: {0} item(s) were still found under the marks, so nothing was changed")]
    Residue(usize),
    /// A mark's overlay text has a character Helvetica can't draw; nothing was redacted.
    #[error("page {page} cannot be redacted safely: {reason}; nothing was changed")]
    Pattern { page: usize, reason: PatternProblem },
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
    /// A stream the redaction created to draw the marks' boxes and labels can't be read back, so
    /// the proof can't tell it apart from content that was there before and nothing can be
    /// exempted from the sweep.
    #[error("a stream the redaction created can't be read back, so the result can't be proven")]
    OverlayUnreadable,
    /// The structure tree couldn't be cleaned completely (#3302): an element written in place that
    /// would need changing, an element whose inherited page can't be found, or a tree too large to
    /// walk. Tags that kept the removed text or annotation would undo the redaction.
    #[error(
        "the document's tags can't be cleaned completely (an element written in place, or a structure tree too large or too deep to follow); the operation was refused"
    )]
    Tags,
    /// Objects written in place nest deeper than hidden information is followed, so its removal
    /// could not be shown complete.
    #[error("the document's objects nest too deeply to remove hidden information completely; the operation was refused")]
    TooDeep,
    /// Past what one redaction may decode an image can't be cleared, and removing it whole would
    /// take what no mark covers with it, unreported.
    #[error("the images under these marks are more than one redaction can decode, so nothing was changed; redact fewer pages at a time")]
    ImagesPastAllowance,
    /// `/JBIG2Globals` used by something removed would stay: something that names them can't be
    /// stored again without them, or something else refers to them. Either way the shapes they
    /// hold, those of marks under the area included, would stay. Counts what can't be stored without
    /// them: 0 when only something else keeps them.
    #[error("{}", shared_jbig2_text(*.0))]
    SharedJbig2Globals(usize),
    /// Finding what shares JBIG2 symbol data with what is being removed would read more of the file
    /// than one run allows (`globals::RUN_BUDGET`): its images and parameters refer to the same
    /// objects in more ways than are checked, or hold more than is decoded. Removing hidden
    /// information says so too when the dictionaries it reads to know copies, or the images it
    /// stores again, would decode more than its image allowance, which no choice of pages makes
    /// smaller.
    #[error(
        "finding and storing again what shares JBIG2 symbol data with something being removed would read more of this file than one run allows, so it can't be shown that the data leaves with it; this was refused"
    )]
    SharedJbig2Budget,
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
    #[error(transparent)]
    Form(#[from] pdfcraft_forms::FormError),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// [`RedactError::SharedJbig2Globals`]'s text, which says what keeps the data.
fn shared_jbig2_text(stuck: usize) -> String {
    let why = match stuck {
        0 => "something else in the file refers to JBIG2 symbol data shared with something being removed".to_string(),
        n => format!("{n} item(s) share JBIG2 symbol data with something being removed and can't be stored without it"),
    };
    format!("{why}, so that data, which can show what was removed, would stay in the file; this was refused")
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
    /// Patterns dropped from a page's resources because nothing painted with them any more.
    pub patterns_removed: usize,
    pub marks: usize,
    pub pages: usize,
    pub glyphs: usize,
    pub images_removed: usize,
    pub images_cleared: usize,
    /// JBIG2 images, anywhere in the document, stored again (as a cleared copy is) without the
    /// `/JBIG2Globals` they shared with something this removed, so those globals leave the file.
    pub images_restored: usize,
    pub paths_removed: usize,
    pub paths_clipped: usize,
    pub forms_rewritten: usize,
    pub forms_removed: usize,
    pub annotations: usize,
    pub fields: usize,
    pub replacement_text: usize,
    /// Structure elements that lost alternate/actual text or emptied marked content, and
    /// structure references to removed annotations.
    pub tags: usize,
    /// The form was an AcroForm+XFA form and its `/XFA` packets (which still held redacted field
    /// values, and from which a relayout could rebuild them) were removed.
    pub xfa_removed: bool,
    /// The content streams this apply added to draw the marks' boxes and labels. They are the
    /// only streams a check for text left under the marks may skip: an input stream labelled
    /// like an overlay (`/PCMark /Redaction`) is checked like any other.
    pub overlays: Vec<ObjRef>,
    /// Document information entries and XMP streams removed because they held redacted text.
    pub metadata: usize,
    /// What became of the PDF/A and PDF/UA identification when the catalog's XMP packet was
    /// removed: written back in a packet of its own, or lost (see [`Identification`]).
    pub identification: Identification,
    /// Places this apply leaves as they are that still hold the removed text (bookmark titles,
    /// attachments, the comments that remain, XMP on other objects), counted so the user can be
    /// told; Remove hidden information removes them. See [`Elsewhere`] for when the counts are
    /// of every place holding any text.
    pub still_holding: Elsewhere,
}

pub(crate) fn rect_of(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// A mark's usable `/QuadPoints` quads ([`pdfcraft_geom::quad_points::usable_quads`]), the set the
/// comment outlines show. Each one is finite, so the box it folds to is a real one.
fn quad_points(doc: &Document, d: &Dict) -> Vec<[f64; 8]> {
    let Some(q) = d.get(b"QuadPoints").map(|q| doc.resolve(q)) else { return Vec::new() };
    let Some(a) = q.as_array() else { return Vec::new() };
    pdfcraft_geom::quad_points::usable_quads(a.iter().map(|x| doc.resolve(x).as_f64()))
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

/// An annotation's `/Subtype`, resolved: it may be an indirect reference (ISO 32000-2 §7.3.10),
/// and the audit reads it that way, so a mark it reports must not be one [`apply`] passes over.
pub(crate) fn subtype(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    d.get(b"Subtype").and_then(|s| doc.resolve(s).as_name().map(<[u8]>::to_vec))
}

/// The pop-up `d` names (`/Popup`), when it is one: it goes with `d`. Anything else it might name
/// (a mark on another page, say) is left alone.
fn named_popup(doc: &Document, d: &Dict) -> Option<ObjRef> {
    let r = d.reference(b"Popup")?;
    doc.get(r).as_dict().is_some_and(|p| subtype(doc, p).as_deref() == Some(b"Popup")).then_some(r)
}

/// Every annotation the pages list. Compared before and after a removal, what is no longer listed
/// leaves the structure tree too ([`tags::drop_object_refs`]).
pub(crate) fn listed_annotations(doc: &Document) -> HashSet<ObjRef> {
    pdfcraft_model::pages(doc).iter().flat_map(|p| annots_of(doc, &p.dict)).filter_map(|a| a.as_ref()).collect()
}

/// Every redaction mark in the document, page by page.
pub fn marks(doc: &Document) -> Vec<Mark> {
    marks_checked(doc).0
}

/// [`marks`], and the 0-based page of each Redact annotation that has no readable area, which
/// [`apply`] refuses rather than skipping.
pub(crate) fn marks_checked(doc: &Document) -> (Vec<Mark>, Vec<usize>) {
    let (mut out, mut unreadable) = (Vec::new(), Vec::new());
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        for a in annots_of(doc, &p.dict) {
            let Some(r) = a.as_ref() else {
                // A mark written in place has no object to find, apply or remove: refuse it
                // rather than apply the others as if it were not there.
                if doc.resolve(&a).as_dict().is_some_and(|d| subtype(doc, d).as_deref() == Some(b"Redact")) {
                    unreadable.push(pi);
                }
                continue;
            };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if subtype(doc, d).as_deref() != Some(b"Redact") {
                continue;
            }
            let mut rects: Vec<[f64; 4]> = quad_points(doc, d)
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
                .collect();
            // No usable quad (all absent, malformed or non-finite): the mark's `/Rect`, else it
            // is unreadable and apply refuses it.
            if rects.is_empty()
                && let Some(r) = rect_of(doc, d.get(b"Rect"))
            {
                rects.push(r);
            }
            if rects.is_empty() {
                unreadable.push(pi);
                continue;
            }
            let overlay = d.get(b"OverlayText").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
            out.push(Mark { page: pi, obj: r, rects, fill: color(doc, d, b"IC"), overlay, look: overlay_look(doc, d) });
        }
    }
    (out, unreadable)
}

/// A page's content streams: (the references or inline objects as listed, their decoded data),
/// decoded within one page's [`pdfcraft_cos::DecodeBudget`].
pub(crate) fn page_streams(doc: &Document, page: &Dict, index: usize) -> Result<(Vec<Object>, Vec<Vec<u8>>), RedactError> {
    page_streams_within(doc, page, index, &mut pdfcraft_cos::DecodeBudget::page())
}

/// [`page_streams`], decoded against `budget`, which the walk of the page's forms then goes on
/// spending.
pub(crate) fn page_streams_within(
    doc: &Document,
    page: &Dict,
    index: usize,
    budget: &mut pdfcraft_cos::DecodeBudget,
) -> Result<(Vec<Object>, Vec<Vec<u8>>), RedactError> {
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
            // Strict: a tolerant decode keeps what precedes corruption and drops the rest, which
            // still draws in viewers that read further; anything not read can't be cleaned.
            Object::Stream(s) => data.push(budget.decode_strict(doc, s).map_err(|_| RedactError::Unreadable(index + 1))?),
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
///
/// # Errors
///
/// On any error `doc` is left as it was: the redaction runs on a copy (cheap: unchanged objects
/// are shared) that replaces `doc` only on success.
pub fn apply(doc: &mut Document, pages: Option<&[usize]>) -> Result<Report, RedactError> {
    let mut work = doc.clone();
    let report = apply_to(&mut work, pages)?;
    *doc = work;
    Ok(report)
}

// Shared appearance stream references can keep a removed annotation's pattern cell reachable
// from an unselected page. Collect only the appearance map and its state streams, not unrelated
// annotation data such as fonts or metadata.
fn appearance_refs(doc: &Document, annotation: &Dict) -> Option<HashSet<ObjRef>> {
    const MAX_REFERENCES: usize = 16_384;
    let mut references = HashSet::new();
    let Some(ap) = annotation.get(b"AP") else { return Some(references) };
    if let Some(reference) = ap.as_ref() {
        references.insert(reference);
    }
    let resolved_ap = doc.resolve(ap);
    let Some(appearances) = resolved_ap.as_dict() else { return Some(references) };
    for key in [b"N".as_slice(), b"R", b"D"] {
        let Some(value) = appearances.get(key) else { continue };
        if let Some(reference) = value.as_ref() {
            references.insert(reference);
        }
        let resolved = doc.resolve(value);
        if let Object::Dict(states) = &*resolved {
            for (_, state) in states.iter() {
                if let Some(reference) = state.as_ref() {
                    references.insert(reference);
                    if references.len() > MAX_REFERENCES {
                        return None;
                    }
                }
            }
        }
    }
    Some(references)
}

/// [`apply`] without the copy: an error can leave `doc` part redacted.
fn apply_to(doc: &mut Document, pages: Option<&[usize]>) -> Result<Report, RedactError> {
    let images = pdfcraft_cos::ImageBudget::new(doc);
    apply_within(doc, pages, images)
}

/// [`apply_to`], its images decoded within `images`.
fn apply_within(doc: &mut Document, pages: Option<&[usize]>, images: pdfcraft_cos::ImageBudget) -> Result<Report, RedactError> {
    let (all_marks, unreadable) = marks_checked(doc);
    // A mark with no readable area is not applied, and applying the others would look complete:
    // refuse before anything changes. Only the pages being applied count.
    let unreadable = unreadable.iter().filter(|page| pages.is_none_or(|p| p.contains(page))).count();
    if unreadable > 0 {
        return Err(RedactError::UnreadableMark(unreadable));
    }
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
    let kept_before = globals::kept(doc)?;
    let mut report = Report { marks: chosen.len(), ..Report::default() };
    // `marks` visits pages in order. Keep slices of that order for appearance generation,
    // content rewriting and verification instead of scanning all marks for every page.
    let groups: Vec<_> = chosen.chunk_by(|a, b| a.page == b.page).filter_map(|group| group.first().map(|mark| (mark.page, group))).collect();
    let by_page: Vec<_> = groups.iter().map(|(page, _)| *page).collect();
    // A pattern pruned from a selected page can remain reachable through another page's
    // resources. When a selected page had patterns, inspect every page after the rewrite.
    let source_pages = pdfcraft_model::pages(doc);
    let shared_pattern_possible = by_page.iter().any(|&pi| {
        source_pages
            .get(pi)
            .and_then(|page| page.dict.get(b"Resources"))
            .map(|resources| doc.resolve(resources))
            .is_some_and(|resources| resources.as_dict().is_some_and(|dict| dict.get(b"Pattern").is_some()))
    });
    // Budget failures must leave even a direct library caller's document untouched.
    let mut remaining = overlay::MAX_BYTES;
    let overlays = groups
        .iter()
        .map(|&(pi, page_marks)| overlay::content(page_marks, &mut remaining).map(|content| (pi, content)))
        .collect::<Result<Vec<_>, _>>()?;
    report.pages = by_page.len();
    localize_resources(doc, &by_page)?;
    let widget_owner: Vec<(ObjRef, ObjRef)> =
        pdfcraft_forms::fields(doc).into_iter().flat_map(|f| f.widgets.iter().map(|w| (w.obj, f.obj)).collect::<Vec<_>>()).collect();
    let listed_before = listed_annotations(doc);
    let mut applying = Applying {
        report,
        doomed_fields: Vec::new(),
        doomed_xobjects: Vec::new(),
        retired_properties: Vec::new(),
        widget_owner,
        removed_text: Vec::new(),
        removed_cut: false,
        removed_room: interp::MAX_REMOVED_BYTES,
        removed_everywhere: HashSet::new(),
        images,
        removed_appearances: HashSet::new(),
        appearance_scan_limit: false,
        pruned_patterns: Vec::new(),
    };
    for &(pi, page_marks) in &groups {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();
        redact_page_content(doc, &mut applying, pi, &page, &rects)?;
        remove_page_annotations(doc, &mut applying, &page, &rects, page_marks)?;
    }
    if applying.images.refused() > 0 {
        return Err(RedactError::ImagesPastAllowance);
    }
    let Applying {
        mut report,
        doomed_fields,
        doomed_xobjects,
        retired_properties,
        mut removed_everywhere,
        removed_text,
        removed_cut,
        mut images,
        mut removed_appearances,
        mut appearance_scan_limit,
        pruned_patterns,
        ..
    } = applying;

    // 2b. Replies and pop-ups of a removed annotation can be listed on another page than their
    // parent: their `/IRT` or `/Parent` would keep the removed comment reachable.
    loop {
        let mut grew = false;
        for p in pdfcraft_model::pages(doc) {
            let annots = annots_of(doc, &p.dict);
            let mut kept = Vec::with_capacity(annots.len());
            for a in &annots {
                // An entry written in place is read as one listed by reference: its `/IRT` or
                // `/Parent` keeps a removed comment reachable all the same, and it has its own text.
                let obj = doc.resolve(a);
                let Some(d) = obj.as_dict() else {
                    kept.push(a.clone());
                    continue;
                };
                let is_reply = d.reference(b"IRT").is_some_and(|x| removed_everywhere.contains(&x)) && subtype(doc, d).as_deref() != Some(b"Widget");
                let is_popup = d.reference(b"Parent").is_some_and(|x| removed_everywhere.contains(&x));
                let Some(r) = a.as_ref() else {
                    if is_reply || is_popup {
                        grew = true;
                        if is_reply {
                            report.annotations += 1;
                        }
                        // Its appearance can be shared with an annotation that stays elsewhere.
                        if !appearance_scan_limit {
                            match appearance_refs(doc, d) {
                                Some(references) => removed_appearances.extend(references),
                                None => appearance_scan_limit = true,
                            }
                            if removed_appearances.len() > 16_384 {
                                appearance_scan_limit = true;
                                removed_appearances.clear();
                            }
                        }
                        // It has no reference to remove elsewhere; the pop-up it names has.
                        if let Some(p) = named_popup(doc, d) {
                            removed_everywhere.insert(p);
                        }
                    } else {
                        kept.push(a.clone());
                    }
                    continue;
                };
                if removed_everywhere.contains(&r) || is_reply || is_popup {
                    // A reply or pop-up removed here can share its appearance with an annotation
                    // that stays on an unselected page, like one removed under a mark: the
                    // all-pages pattern check must see it.
                    if !appearance_scan_limit {
                        match appearance_refs(doc, d) {
                            Some(references) => removed_appearances.extend(references),
                            None => appearance_scan_limit = true,
                        }
                        if removed_appearances.len() > 16_384 {
                            appearance_scan_limit = true;
                            removed_appearances.clear();
                        }
                    }
                    if removed_everywhere.insert(r) {
                        grew = true;
                        if is_reply {
                            report.annotations += 1;
                        }
                    }
                } else {
                    kept.push(a.clone());
                }
            }
            if kept.len() != annots.len() {
                doc.update_dict(p.obj, |d| {
                    if kept.is_empty() {
                        d.remove(b"Annots");
                    } else {
                        d.set(b"Annots".to_vec(), Object::Array(kept));
                    }
                })?;
            }
        }
        if !grew {
            break;
        }
    }

    // 3. Form fields with a widget under a mark (all their widgets go).
    for field in &doomed_fields {
        if pdfcraft_forms::fields(doc).iter().any(|f| f.obj == *field) {
            pdfcraft_forms::delete_field_object(doc, *field)?;
            report.fields += 1;
        }
    }
    // A widget written in place on another page names its deleted field through `/Parent`, which
    // would keep the field and its value in the file: it goes with the field.
    if !doomed_fields.is_empty() {
        for p in pdfcraft_model::pages(doc) {
            let annots = annots_of(doc, &p.dict);
            let kept: Vec<Object> = annots
                .iter()
                .filter(|a| {
                    a.as_ref().is_some() || !matches!(&**a, Object::Dict(d) if d.reference(b"Parent").is_some_and(|f| doomed_fields.contains(&f)))
                })
                .cloned()
                .collect();
            if kept.len() != annots.len() {
                report.annotations += annots.len() - kept.len();
                doc.update_dict(p.obj, |d| {
                    if kept.is_empty() {
                        d.remove(b"Annots");
                    } else {
                        d.set(b"Annots".to_vec(), Object::Array(kept));
                    }
                })?;
            }
        }
    }

    // 3a. A tagged PDF's structure tree still points at the removed comments and widgets
    // (`/OBJR`, #2874): drop those references, or a full rewrite keeps them reachable, the
    // comment text and the field value included. Widgets of a removed field on other pages count.
    let gone: HashSet<ObjRef> = listed_before.difference(&listed_annotations(doc)).copied().collect();
    report.tags += tags::drop_object_refs(doc, &gone)?;

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
            report.overlays.push(pdfcraft_edit::stamp(doc, pi, "Redaction", content)?);
        }
    }

    // A removed image or form must not stay reachable through another resource dictionary (an
    // inherited or shared /Resources): a full save keeps whatever is still referenced.
    interp::unlink_doomed(doc, &doomed_xobjects, &retired_properties)?;
    // Likewise a pruned pattern: a /Pages node above the page can still name its cell.
    interp::unlink_pruned_patterns(doc, &pruned_patterns)?;

    // 5. Verify: nothing readable may remain under a region.
    let mut residue = 0;
    for &(pi, page_marks) in &groups {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();
        let mut budget = pdfcraft_cos::DecodeBudget::page();
        let (list, data) = page_streams_within(doc, &page.dict, pi, &mut budget)?;
        // The overlay stream draws text of its own: the overlay label is allowed.
        let original: Vec<Vec<u8>> =
            list.iter().zip(data).filter(|(o, _)| !o.as_ref().is_some_and(|r| report.overlays.contains(&r))).map(|(_, d)| d).collect();
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut scratch = Report::default();
        // Verifying doesn't decode images: what clearing left is read as the page draws it.
        let mut images = pdfcraft_cos::ImageBudget::with_work(0);
        let mut scope = Scope::new(&rects, Mode::Verify, &mut scratch, budget, &mut images);
        residue += process(doc, &mut scope, &original, &resources, pdfcraft_content::Matrix::IDENTITY).residue;
    }
    if residue > 0 {
        return Err(RedactError::Residue(residue));
    }
    // The interpreter can remove a pattern's paint while another use keeps the cell reachable.
    // Inspect the finished selected pages, including rewritten Forms, and all pages if a removed
    // pattern or appearance may still be shared. Retained unsafe cells refuse the redaction;
    // `apply` runs on a copy, so the refusal leaves the caller's document as it was.
    let shared_appearance_possible = appearance_scan_limit
        || !removed_appearances.is_empty()
            && pdfcraft_model::pages(doc).iter().enumerate().any(|(index, page)| {
                !by_page.contains(&index)
                    && annots_of(doc, &page.dict).iter().any(|annotation| {
                        doc.resolve(annotation).as_dict().is_some_and(|dict| {
                            appearance_refs(doc, dict).is_none_or(|refs| refs.iter().any(|reference| removed_appearances.contains(reference)))
                        })
                    })
            });
    let first_page = by_page.first().map_or(1, |p| p + 1);
    let check_pages =
        if shared_pattern_possible || shared_appearance_possible { (0..pdfcraft_model::pages(doc).len()).collect::<Vec<_>>() } else { by_page };
    // A pruned pattern may also stay named by a /Pages node no page inherits from, or by one a
    // page with a broken /Parent link still reaches: then every page-tree node is inspected too.
    pattern_guard::check(doc, &check_pages, shared_pattern_possible || !pruned_patterns.is_empty())?;
    // Whatever reference keeps a pruned pattern in the file -- one no walk above follows -- the
    // full save would write it: prove it safe or refuse.
    pattern_guard::check_pruned_unreachable(doc, &pruned_patterns, first_page)?;
    // 6. The document's metadata must not keep what the pages no longer show (#2448).
    let scrubbed = metadata::scrub(doc, &removed_text, !removed_cut)?;
    report.metadata = scrubbed.removed;
    report.identification = scrubbed.identification;
    // Last of what leaves: an image that left with any of the above, the metadata removed just
    // now included, is a user of a dictionary too. Storing images again changes no page's content
    // or pattern cells, so the checks above hold for what follows.
    report.images_restored = globals::unshare(doc, &kept_before, &mut images)?;
    // What it doesn't scrub is counted, never changed.
    report.still_holding = elsewhere::find(doc, &removed_text, !removed_cut);
    // The previous revision still holds the removed content: the next save must rewrite, and the
    // rewritten file is a different file from its source, so it gets a new identifier.
    doc.require_full_save_with_new_id();
    Ok(report)
}

/// What applying the marks collects across pages.
struct Applying {
    report: Report,
    doomed_fields: Vec<ObjRef>,
    /// The original image and form XObjects the rewrite drops or replaces, on any page.
    doomed_xobjects: Vec<ObjRef>,
    retired_properties: Vec<Object>,
    widget_owner: Vec<(ObjRef, ObjRef)>,
    removed_everywhere: HashSet<ObjRef>,
    /// The text of the removed glyphs, for the metadata check at the end.
    removed_text: Vec<String>,
    /// Some removed text was too long to keep whole for that check.
    removed_cut: bool,
    /// Bytes of removed text later pages may still keep (`interp::MAX_REMOVED_BYTES` for the
    /// document).
    removed_room: usize,
    /// What clearing images may spend, across the run's pages.
    images: pdfcraft_cos::ImageBudget,
    /// Appearance streams of the annotations removed, on any page: one shared with a kept
    /// annotation keeps a pruned pattern's cell reachable (see `pattern_guard`).
    removed_appearances: HashSet<ObjRef>,
    /// Too many appearance references to track: the pattern guard inspects every page.
    appearance_scan_limit: bool,
    /// Pattern objects pruned from a page's resources, which page-tree nodes may still name.
    pruned_patterns: Vec<ObjRef>,
}

/// Step 1 for one page: rewrites its content so nothing readable stays under a mark, and cleans the tags it no longer shows.
fn redact_page_content(
    doc: &mut Document,
    applying: &mut Applying,
    pi: usize,
    page: &pdfcraft_model::Page,
    rects: &[[f64; 4]],
) -> Result<(), RedactError> {
    let Applying { report, doomed_xobjects, retired_properties, removed_text, removed_cut, removed_room, images, pruned_patterns, .. } = applying;
    // 1. Content.
    let mut budget = pdfcraft_cos::DecodeBudget::page();
    let (list, data) = page_streams_within(doc, &page.dict, pi, &mut budget)?;
    let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    // What the walk leaves of the page's allowance, the cleanups below spend.
    let (out, mut budget) = {
        let mut scope = Scope::new(rects, Mode::Apply, report, budget, images).with_removed_room(*removed_room);
        let out = process(doc, &mut scope, &data, &resources, pdfcraft_content::Matrix::IDENTITY);
        scope.flush();
        removed_text.append(&mut scope.removed_text);
        *removed_cut |= scope.removed_cut;
        *removed_room = scope.removed_room;
        (out, scope.into_budget())
    };
    // Fail closed: the interpreter lays vertical-writing text out horizontally, so it cannot
    // tell whether those glyphs lie under a mark. Rather than report success and possibly
    // leave the text in place, refuse the whole operation (the caller keeps the previous
    // document). The error does not claim the text was under the mark. Full vertical-text
    // removal is a future enhancement.
    if let Some(error) = out.error {
        return Err(error.into());
    }
    if out.vertical {
        return Err(RedactError::VerticalText(pi + 1));
    }
    // An inline image the renderer reads no image for (no `EI` it accepts, or parameters it
    // can't read): where the page's operators resume after it is a guess, so neither removal
    // nor the verify pass can be trusted on this page. Likewise bytes the parser can't read in
    // what the walk read, the page's content or a form under a mark: text they hide is neither
    // found nor removed, and other readers may draw it.
    if out.ambiguous || out.unparsed {
        return Err(RedactError::Unreadable(pi + 1));
    }
    doomed_xobjects.extend_from_slice(&out.doomed);
    retired_properties.extend(out.retired_properties);
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
            // The rewritten XObjects first, as for a rewritten form: one left without resources of
            // its own draws through these, and `prune` follows it only under a name it can find.
            for (n, r) in &out.xobjects {
                xo.set(n.clone(), Object::Ref(*r));
            }
            // A full save keeps whatever is still referenced: unlink the originals this page no
            // longer draws so the removed image or form bytes cannot survive in the saved file.
            let page_content: Vec<Vec<u8>> = new_list
                .iter()
                .zip(&list)
                .zip(&data)
                .map(|((now, was), read)| match &*doc.resolve(now) {
                    // Left as it was: what was read above, with its parameters by reference.
                    _ if now == was => read.clone(),
                    // Written above by `Stream::flate`, which leaves no `/DecodeParms`.
                    Object::Stream(s) => s.decoded().unwrap_or_default(),
                    _ => Vec::new(),
                })
                .collect();
            interp::prune(doc, &mut budget, &mut xo, &out.replaced, &page_content)?;
            res.set(b"XObject".to_vec(), Object::Dict(xo));
        }
        let (_, cleaned_data) = page_streams(
            doc,
            &{
                let mut cleaned_page = page.dict.clone();
                cleaned_page.set(b"Contents".to_vec(), Object::Array(new_list.clone()));
                cleaned_page
            },
            pi,
        )?;
        // A pattern this page selected but no surviving paint draws leaves its resources, so
        // its cell -- a content stream this pass does not enter -- cannot stay in the file.
        // A Type 3 glyph without resources paints with patterns nobody tracked: keep them all.
        let painted = if out.keeps_all_patterns { &out.patterns_seen } else { &out.patterns_painted };
        report.patterns_removed += interp::prune_patterns(doc, &mut res, &out.patterns_seen, painted, pruned_patterns);
        retired_properties.extend(interp::prune_properties(doc, &mut budget, &mut res, &cleaned_data)?);
        type3::prune_type3_from_resources(doc, &mut budget, &mut res, &cleaned_data, Some(&page.dict))?;
        doc.update_dict(page.obj, |d| {
            d.set(b"Contents".to_vec(), Object::Array(new_list));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        })?;
        // Tags must not keep what the page no longer shows.
        let after = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
        let (_, new_data) = page_streams(doc, &after, pi)?;
        let (changed_ids, empty_ids) =
            tags::touched(doc, &resources, pdfcraft_content::Pieces::join(&data).data(), pdfcraft_content::Pieces::join(&new_data).data());
        report.tags += tags::clean(doc, page.obj, None, &changed_ids, &empty_ids)?;
    }

    for (old, new, changed, empty) in out.tag_changes {
        report.tags += tags::clean(doc, page.obj, Some((old, new)), &changed, &empty)?;
    }
    Ok(())
}

/// Step 2 for one page: the marks, and whatever lies under them (with their pop-ups).
fn remove_page_annotations(
    doc: &mut Document,
    applying: &mut Applying,
    page: &pdfcraft_model::Page,
    rects: &[[f64; 4]],
    page_marks: &[&Mark],
) -> Result<(), RedactError> {
    let Applying { report, doomed_fields, widget_owner, removed_everywhere, removed_appearances, appearance_scan_limit, .. } = applying;
    // 2. Annotations: the marks, and whatever lies under them (with their pop-ups). An entry
    // written in place (a dictionary in `/Annots`, not a reference) is judged like any other: its
    // `/Contents` and `/IRT` stay in the file for as long as the page lists it.
    let annots = annots_of(doc, &doc.get(page.obj).as_dict().cloned().unwrap_or_default());
    /// What decides an entry's fate, read once.
    struct Entry {
        r: Option<ObjRef>,
        widget: bool,
        irt: Option<ObjRef>,
        parent: Option<ObjRef>,
        popup: Option<ObjRef>,
    }
    let entries: Vec<Option<Entry>> = annots
        .iter()
        .map(|a| {
            let obj = doc.resolve(a);
            let d = obj.as_dict()?;
            Some(Entry {
                r: a.as_ref(),
                widget: subtype(doc, d).as_deref() == Some(b"Widget"),
                irt: d.reference(b"IRT"),
                parent: d.reference(b"Parent"),
                popup: named_popup(doc, d),
            })
        })
        .collect();
    let mut removed: Vec<ObjRef> = Vec::new();
    // Entries written in place that go, by position in `/Annots`.
    let mut inline_gone = vec![false; annots.len()];
    for (i, a) in annots.iter().enumerate() {
        let obj = doc.resolve(a);
        let Some(d) = obj.as_dict() else { continue };
        let r = a.as_ref();
        let kind = subtype(doc, d).unwrap_or_default();
        if kind == b"Redact" {
            // A mark written in place was refused before anything changed.
            if let Some(r) = r
                && page_marks.iter().any(|m| m.obj == r)
            {
                removed.push(r);
            }
            continue;
        }
        if kind == b"Popup" {
            continue;
        }
        if rect_of(doc, d.get(b"Rect")).is_some_and(|b| rects.iter().any(|x| pdfcraft_content::overlaps(*x, b, 0.0))) {
            match r {
                Some(r) => removed.push(r),
                None => inline_gone[i] = true,
            }
            if kind == b"Widget" {
                // A widget written in place has no reference of its own; its `/Parent` is the field,
                // even one no referenced widget belongs to (it then has no entry in `widget_owner`).
                // Step 3 deletes only what the form lists as a field, so any `/Parent` is safe here.
                let owner = r.and_then(|r| widget_owner.iter().find(|(w, _)| *w == r).map(|(_, f)| *f)).or_else(|| d.reference(b"Parent"));
                if let Some(field) = owner
                    && !doomed_fields.contains(&field)
                {
                    doomed_fields.push(field);
                }
            } else {
                report.annotations += 1;
            }
        }
    }
    // Replies (`/IRT`, ISO 32000-2 §12.5.6.2) to a removed annotation go too, and replies to
    // those: a reply placed elsewhere would keep its parent — the comment and the QuadPoints
    // of the redacted words — in the file through its `/IRT` (#2659). Each pass removes at
    // least one more annotation of this page or stops, so it ends.
    loop {
        let mut grew = false;
        for (i, e) in entries.iter().enumerate() {
            let Some(e) = e else { continue };
            let gone = e.r.map_or(inline_gone[i], |r| removed.contains(&r));
            if gone || e.widget || !e.irt.is_some_and(|p| removed.contains(&p)) {
                continue;
            }
            match e.r {
                Some(r) => removed.push(r),
                None => inline_gone[i] = true,
            }
            report.annotations += 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    // A removed annotation's appearance streams can be shared with a kept one on another page,
    // which would keep a pruned pattern's cell reachable: remember them for the pattern guard.
    for reference in &removed {
        if *appearance_scan_limit {
            break;
        }
        if let Some(annotation) = doc.get(*reference).as_dict() {
            match appearance_refs(doc, annotation) {
                Some(references) => {
                    removed_appearances.extend(references);
                    if removed_appearances.len() > 16_384 {
                        *appearance_scan_limit = true;
                        removed_appearances.clear();
                    }
                }
                None => *appearance_scan_limit = true,
            }
        }
    }
    // An entry written in place that goes keeps no reference, but its appearances count as well.
    for (i, a) in annots.iter().enumerate() {
        if *appearance_scan_limit {
            break;
        }
        if let (true, Some(annotation)) = (inline_gone[i], doc.resolve(a).as_dict()) {
            match appearance_refs(doc, annotation) {
                Some(references) => removed_appearances.extend(references),
                None => *appearance_scan_limit = true,
            }
            if removed_appearances.len() > 16_384 {
                *appearance_scan_limit = true;
                removed_appearances.clear();
            }
        }
    }
    // Pop-ups go with their annotation: those that name it (`/Parent`), and those it names
    // (`/Popup`), the only link an annotation written in place can have to its pop-up.
    let mut going: HashSet<ObjRef> = removed.iter().copied().collect();
    for (i, e) in entries.iter().enumerate() {
        let Some(e) = e else { continue };
        let gone = e.r.map_or(inline_gone[i], |r| removed.contains(&r));
        if gone {
            going.extend(e.popup);
        }
    }
    for (i, e) in entries.iter().enumerate() {
        let Some(e) = e else { continue };
        if e.parent.is_some_and(|p| removed.contains(&p)) {
            match e.r {
                Some(r) => {
                    going.insert(r);
                }
                None => inline_gone[i] = true,
            }
        }
    }
    // Pop-ups dropped with their parent count too: a reply on another page may answer one.
    removed_everywhere.extend(going.iter().copied());
    let kept: Vec<Object> = annots
        .into_iter()
        .enumerate()
        .filter(|(i, a)| match a.as_ref() {
            Some(r) => !going.contains(&r),
            None => !inline_gone[*i],
        })
        .map(|(_, a)| a)
        .collect();
    doc.update_dict(page.obj, |d| {
        if kept.is_empty() {
            d.remove(b"Annots");
        } else {
            d.set(b"Annots".to_vec(), Object::Array(kept));
        }
    })?;
    Ok(())
}

/// Remove redaction marks without applying them (`None` = all), with their replies and pop-ups as
/// deleting a comment does: a reply left behind would keep its mark in the file through `/IRT`.
/// Returns how many marks went.
pub fn clear_marks(doc: &mut Document, pages: Option<&[usize]>) -> Result<usize, RedactError> {
    let targets: HashSet<ObjRef> = marks(doc).iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).map(|m| m.obj).collect();
    if targets.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    let listed: Vec<Object> = pdfcraft_model::pages(doc).iter().flat_map(|p| annots_of(doc, &p.dict)).collect();
    let doomed = pdfcraft_annot::with_replies_and_popups(doc, &listed, targets.iter().copied());
    for p in pdfcraft_model::pages(doc) {
        let annots = annots_of(doc, &p.dict);
        let kept: Vec<Object> = annots.iter().filter(|a| !pdfcraft_annot::goes_with(doc, a, &doomed)).cloned().collect();
        // A group subordinate of a mark stays, as when a comment's group primary is deleted, and
        // loses its `/IRT`: through it the mark would stay in the file.
        let kept = pdfcraft_annot::release_members(doc, kept, &doomed)?;
        // As deleting a comment does: a page whose marks were all it had loses `/Annots` again,
        // and an indirect array stays the one object the page points to. A page whose inline
        // member lost its `/IRT` is written too.
        if kept != annots {
            pdfcraft_annot::set_annots(doc, p.obj, kept)?;
        }
    }
    Ok(targets.len())
}

/// The name to save a document under after [`apply`]: `_Redacted` before the extension, as
/// Acrobat appends it ("contract.pdf" gives "contract_Redacted.pdf"). The `.pdf` extension is
/// matched in any letter case. A name that already ends in `_Redacted` keeps it once.
pub fn redacted_file_name(name: &str) -> String {
    let cut = name.len().saturating_sub(4);
    let stem = match (name.get(..cut), name.get(cut..)) {
        (Some(stem), Some(ext)) if ext.eq_ignore_ascii_case(".pdf") => stem,
        _ => name,
    };
    if stem.ends_with("_Redacted") { format!("{stem}.pdf") } else { format!("{stem}_Redacted.pdf") }
}
