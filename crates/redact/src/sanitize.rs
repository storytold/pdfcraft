//! Remove Hidden Information and Sanitize Document (architecture §11.1, execution plan M8.4).
//!
//! [`scan`] counts what each category would remove; [`remove_hidden`] removes the chosen
//! categories; [`sanitize`] removes all of them. Every removal makes the next save a full
//! rewrite, so the earlier revision (which still holds the data) leaves the file.

use std::collections::HashSet;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::interp::{Mode, Scope, decoded_contents, process, rebuild_resources};
use crate::{RedactError, Report, annots_of, page_streams, put_annots};

/// The categories of Acrobat's Remove Hidden Information panel that PdfCraft handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hidden {
    /// Document information (`/Info`) and XMP metadata streams, on any object.
    Metadata,
    /// Embedded files, file attachment annotations, associated files (`/AF`, wherever they
    /// hang), multimedia annotations (Screen, Movie, RichMedia, 3D: their media, assets, models
    /// and file paths) and renditions.
    Attachments,
    /// Comments and markup annotations (with their pop-ups).
    Comments,
    /// Interactive form fields (flattened: their appearance stays as page content; Hidden and
    /// NoView ones go with nothing drawn).
    FormFields,
    /// Text drawn invisibly (render modes 3 and 7) or wholly off the page.
    HiddenText,
    /// Optional content (layers) that is off: its content, then the layers themselves.
    HiddenLayers,
    Bookmarks,
    /// Links, actions (open action, additional actions, every annotation's), document
    /// JavaScript, slideshows (`/AlternatePresentations`) and 3D instantiation scripts.
    LinksActionsScripts,
    /// Private data of other applications (`/PieceInfo`, on any object).
    PrivateData,
}

pub const HIDDEN: [Hidden; 9] = [
    Hidden::Metadata,
    Hidden::Attachments,
    Hidden::Comments,
    Hidden::FormFields,
    Hidden::HiddenText,
    Hidden::HiddenLayers,
    Hidden::Bookmarks,
    Hidden::LinksActionsScripts,
    Hidden::PrivateData,
];

impl Hidden {
    pub fn label(self) -> &'static str {
        match self {
            Hidden::Metadata => "Metadata",
            Hidden::Attachments => "File attachments",
            Hidden::Comments => "Comments and markups",
            Hidden::FormFields => "Form fields",
            Hidden::HiddenText => "Hidden text",
            Hidden::HiddenLayers => "Hidden layers",
            Hidden::Bookmarks => "Bookmarks",
            Hidden::LinksActionsScripts => "Links, actions and JavaScripts",
            Hidden::PrivateData => "Private data of other applications",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Hidden::Metadata => "metadata",
            Hidden::Attachments => "attachments",
            Hidden::Comments => "comments",
            Hidden::FormFields => "form-fields",
            Hidden::HiddenText => "hidden-text",
            Hidden::HiddenLayers => "hidden-layers",
            Hidden::Bookmarks => "bookmarks",
            Hidden::LinksActionsScripts => "links-actions-scripts",
            Hidden::PrivateData => "private-data",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        HIDDEN.into_iter().find(|h| h.id() == id)
    }
}

fn catalog(doc: &Document) -> Option<(ObjRef, Dict)> {
    let r = doc.root()?;
    Some((r, doc.get(r).as_dict()?.clone()))
}

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Option<Dict> {
    d.get(key).and_then(|o| doc.resolve(o).as_dict().cloned())
}

/// Entries of a name tree (`/Names` leaves, `/Kids` recursion), bounded against cycles.
fn name_tree_len(doc: &Document, node: &Dict, depth: usize) -> usize {
    if depth > 32 {
        return 0;
    }
    let leaves = node.get(b"Names").and_then(|n| doc.resolve(n).as_array().map(|a| a.len() / 2)).unwrap_or(0);
    let kids = node.get(b"Kids").and_then(|k| doc.resolve(k).as_array().cloned()).unwrap_or_default();
    leaves + kids.iter().filter_map(|k| doc.resolve(k).as_dict().cloned()).map(|k| name_tree_len(doc, &k, depth + 1)).sum::<usize>()
}

fn outline_len(doc: &Document, first: Option<&Object>, depth: usize) -> usize {
    let mut n = 0;
    let mut cur = first.cloned();
    let mut guard = 0;
    while let Some(o) = cur {
        guard += 1;
        if guard > 100_000 || depth > 64 {
            break;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { break };
        n += 1 + outline_len(doc, d.get(b"First"), depth + 1);
        cur = d.get(b"Next").cloned();
    }
    n
}

/// Every indirect object reachable from the catalog, each once, with its current generation.
/// Cycles are cut by the seen-set, which holds at most one entry per object number (no more
/// than the cross-reference table already does); nesting inside one object is bounded by the
/// parser.
fn reachable(doc: &Document) -> Vec<ObjRef> {
    let mut out = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<ObjRef> = doc.root().into_iter().collect();
    while let Some(r) = stack.pop() {
        if !seen.insert(r.num) {
            continue;
        }
        let o = doc.get(r);
        if matches!(*o, Object::Null) {
            continue;
        }
        refs_in(&o, &mut |x| stack.push(x));
        out.push(ObjRef::new(r.num, doc.generation(r.num)));
    }
    out
}

fn refs_in(o: &Object, f: &mut impl FnMut(ObjRef)) {
    match o {
        Object::Ref(r) => f(*r),
        Object::Array(a) => a.iter().for_each(|x| refs_in(x, f)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| refs_in(v, f)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| refs_in(v, f)),
        _ => {}
    }
}

/// What one occurrence of a key counts for (see [`count_key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Count {
    /// One per dictionary that holds the key.
    Dicts,
    /// One per item of the array under the key (one per associated file); a value that is not an
    /// array, or an empty one, counts as one.
    Entries,
}

/// The keys that may sit on any dictionary (the catalog, a page, an XObject, a structure element,
/// an annotation…) and are counted and stripped wherever they are: the category each belongs to,
/// and what an occurrence counts for. XMP metadata (ISO 32000-2 §14.3.2), associated files
/// (§14.13) and the private data of other applications (§14.5).
const ANYWHERE: [(Hidden, &[u8], Count); 3] =
    [(Hidden::Metadata, b"Metadata", Count::Dicts), (Hidden::Attachments, b"AF", Count::Entries), (Hidden::PrivateData, b"PieceInfo", Count::Dicts)];

/// How many times `o` (its own dictionary, and the direct ones inside it) holds `key`, each
/// occurrence counted as `count` says.
fn count_key(doc: &Document, o: &Object, key: &[u8], count: Count, depth: usize) -> usize {
    if depth > 64 {
        return 0;
    }
    let in_dict = |d: &Dict| -> usize {
        let own = match (d.get(key), count) {
            (None, _) => 0,
            (Some(v), Count::Entries) => doc.resolve(v).as_array().map_or(1, |a| a.len().max(1)),
            (Some(_), Count::Dicts) => 1,
        };
        d.iter().fold(own, |n, (_, v)| n.saturating_add(count_key(doc, v, key, count, depth + 1)))
    };
    match o {
        Object::Dict(d) => in_dict(d),
        Object::Stream(s) => in_dict(&s.dict),
        Object::Array(a) => a.iter().fold(0, |n, x| n.saturating_add(count_key(doc, x, key, count, depth + 1))),
        _ => 0,
    }
}

/// `o` without `keys` in its dictionary or in any direct dictionary inside it (and without the
/// /LastModified that goes with a removed /PieceInfo); `None` when it holds none of them.
fn strip_keys(o: &Object, keys: &[&[u8]], depth: usize) -> Option<Object> {
    if depth > 64 {
        return None;
    }
    match o {
        Object::Dict(d) => strip_dict(d, keys, depth).map(Object::Dict),
        Object::Stream(s) => strip_dict(&s.dict, keys, depth).map(|dict| Object::Stream(pdfcraft_cos::Stream { dict, raw: s.raw.clone() })),
        Object::Array(a) => {
            let mut changed = false;
            let mut items = Vec::with_capacity(a.len());
            for x in a {
                match strip_keys(x, keys, depth + 1) {
                    Some(y) => {
                        changed = true;
                        items.push(y);
                    }
                    None => items.push(x.clone()),
                }
            }
            changed.then_some(Object::Array(items))
        }
        _ => None,
    }
}

fn strip_dict(d: &Dict, keys: &[&[u8]], depth: usize) -> Option<Dict> {
    let piece_goes = keys.contains(&&b"PieceInfo"[..]) && d.contains(b"PieceInfo");
    let mut out = Dict::new();
    let mut changed = false;
    for (k, v) in d.iter() {
        if keys.contains(&k.as_slice()) || (piece_goes && k == b"LastModified") {
            changed = true;
            continue;
        }
        match strip_keys(v, keys, depth + 1) {
            Some(y) => {
                changed = true;
                out.set(k.clone(), y);
            }
            None => out.set(k.clone(), v.clone()),
        }
    }
    changed.then_some(out)
}

/// A comment as the comments list sees it, less file attachments (removed with Attachments) and
/// redaction marks.
fn is_comment(subtype: &[u8]) -> bool {
    !matches!(subtype, b"FileAttachment" | b"Redact") && pdfcraft_annot::is_comment_subtype(&String::from_utf8_lossy(subtype))
}

/// An annotation that embeds or names media content (ISO 32000-2 §12.5.6.17–19, §13.6–13.7):
/// removed with Attachments, since what it holds (media, assets, 3D models, file paths) is
/// embedded content the user cannot see on the page.
fn is_multimedia(subtype: &[u8]) -> bool {
    matches!(subtype, b"Screen" | b"Movie" | b"RichMedia" | b"3D")
}

/// The 3D stream of a 3D annotation: `/3DD` is the stream, or a 3D reference dictionary to it.
fn three_d_stream(doc: &Document, annot: &Dict) -> Option<ObjRef> {
    let via_ref = |d: &Dict| d.reference(b"3DD").filter(|s| matches!(&*doc.get(*s), Object::Stream(_)));
    match annot.get(b"3DD")? {
        Object::Ref(r) => match &*doc.get(*r) {
            Object::Stream(_) => Some(*r),
            Object::Dict(d) => via_ref(d),
            _ => None,
        },
        Object::Dict(d) => via_ref(d),
        _ => None,
    }
}

/// The 3D stream of a 3D annotation when it runs a script as it is instantiated
/// (`/OnInstantiate`); `None` for a 3D annotation whose stream has no script, and for any other
/// annotation.
fn scripted_3d_stream(doc: &Document, annot: &Dict) -> Option<ObjRef> {
    if annot.name(b"Subtype") != Some(b"3D") {
        return None;
    }
    three_d_stream(doc, annot).filter(|s| doc.get(*s).as_dict().is_some_and(|d| d.contains(b"OnInstantiate")))
}

/// The layers that are off in the default configuration.
fn hidden_layers(doc: &Document) -> Vec<ObjRef> {
    let Some((_, cat)) = catalog(doc) else { return Vec::new() };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return Vec::new() };
    let Some(d) = sub(doc, &oc, b"D") else { return Vec::new() };
    let all: Vec<ObjRef> =
        oc.get(b"OCGs").and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default();
    let list = |k: &[u8]| -> Vec<ObjRef> {
        d.get(k).and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
    };
    let (on, off) = (list(b"ON"), list(b"OFF"));
    if d.name(b"BaseState") == Some(b"OFF") {
        all.into_iter().filter(|g| !on.contains(g)).collect()
    } else {
        off.into_iter().filter(|g| all.contains(g)).collect()
    }
}

/// Glyphs of hidden text, or content of hidden layers, on every page (`write` = remove them).
fn content_pass(doc: &mut Document, text: bool, layers: bool, write: bool) -> Result<usize, RedactError> {
    let off = if layers { hidden_layers(doc) } else { Vec::new() };
    if !text && off.is_empty() {
        return Ok(0);
    }
    let mut total = 0;
    // The original image and form XObjects the rewrite drops or replaces, on any page.
    let mut doomed = Vec::new();
    let n = pdfcraft_model::pages(doc).len();
    for pi in 0..n {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let Ok((list, data)) = page_streams(doc, &page.dict, pi) else {
            if write {
                return Err(RedactError::Unreadable(pi + 1));
            }
            continue;
        };
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut report = Report::default();
        let mode = if write { Mode::Apply } else { Mode::Verify };
        let mut scope = Scope::new(&[], mode, &mut report);
        if text {
            let c = page.crop(doc);
            scope.hidden_text = Some([c[0].min(c[2]), c[1].min(c[3]), c[0].max(c[2]), c[1].max(c[3])]);
        }
        scope.hidden_layers = off.clone();
        let out = process(doc, &mut scope, &data, &resources, Matrix::IDENTITY);
        let blocks = scope.layer_blocks;
        total += out.residue + blocks + report.glyphs;
        if !write {
            continue;
        }
        doomed.extend_from_slice(&out.doomed);
        let mut new_list = list.clone();
        let mut changed = false;
        for (i, new) in out.streams.iter().enumerate() {
            let Some(bytes) = new else { continue };
            let mut dict = match &*doc.resolve(&list[i]) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            new_list[i] = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict, bytes))));
            changed = true;
        }
        if changed || !out.xobjects.is_empty() || !out.replaced.is_empty() {
            // A full save keeps whatever is still referenced: the originals this page no longer
            // draws leave its resources, and so do the off layers' property lists with their blocks
            // (so the group dictionaries and their names stop being reachable through the page).
            // The page stores its resources whole, changed or not.
            let mut res = resources;
            rebuild_resources(doc, &mut res, &out.rewrite(), Some(&decoded_contents(doc, &new_list)), &off);
            doc.update_dict(page.obj, |d| {
                d.set(b"Contents".to_vec(), Object::Array(new_list));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
        }
    }
    // What a page no longer draws must not stay reachable through an inherited or shared
    // /Resources either.
    crate::interp::unlink_doomed(doc, &doomed)?;
    Ok(total)
}

/// How many items each category would remove (categories with nothing are included as 0).
pub fn scan(doc: &Document) -> Vec<(Hidden, usize)> {
    let cat = catalog(doc).map(|c| c.1).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let annots: Vec<Dict> = pages.iter().flat_map(|p| annots_of(doc, &p.dict)).filter_map(|a| doc.resolve(&a).as_dict().cloned()).collect();
    let count_sub = |s: &[u8]| annots.iter().filter(|a| a.name(b"Subtype") == Some(s)).count();
    let names = sub(doc, &cat, b"Names").unwrap_or_default();
    // XMP, associated files and private data can sit on any dictionary (an XObject, a structure
    // element, an annotation…): count them wherever they are reachable from the catalog.
    let mut anywhere = [0usize; ANYWHERE.len()];
    for r in reachable(doc) {
        let o = doc.get(r);
        for (n, (_, key, count)) in anywhere.iter_mut().zip(ANYWHERE) {
            *n = n.saturating_add(count_key(doc, &o, key, count, 0));
        }
    }
    // What a category has under those keys (nothing, for most).
    let found_anywhere =
        |h: Hidden| ANYWHERE.iter().zip(anywhere).filter(|((category, ..), _)| *category == h).fold(0usize, |sum, (_, n)| sum.saturating_add(n));
    let mut doc2 = doc.clone();
    HIDDEN
        .into_iter()
        .map(|h| {
            let n = match h {
                Hidden::Metadata => doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().map(|d| d.len())).unwrap_or(0),
                Hidden::Attachments => sub(doc, &names, b"EmbeddedFiles")
                    .map_or(0, |t| name_tree_len(doc, &t, 0))
                    .saturating_add(count_sub(b"FileAttachment"))
                    .saturating_add(annots.iter().filter(|a| is_multimedia(a.name(b"Subtype").unwrap_or(b""))).count())
                    .saturating_add(sub(doc, &names, b"Renditions").map_or(0, |t| name_tree_len(doc, &t, 0))),
                Hidden::Comments => annots.iter().filter(|a| is_comment(a.name(b"Subtype").unwrap_or(b""))).count(),
                Hidden::FormFields => pdfcraft_forms::fields(doc).len(),
                Hidden::HiddenText => content_pass(&mut doc2, true, false, false).unwrap_or(0),
                Hidden::HiddenLayers => {
                    let off = hidden_layers(doc).len();
                    if off == 0 { 0 } else { off + content_pass(&mut doc2, false, true, false).unwrap_or(0) }
                }
                Hidden::Bookmarks => sub(doc, &cat, b"Outlines").map_or(0, |o| outline_len(doc, o.get(b"First"), 0)),
                Hidden::LinksActionsScripts => {
                    count_sub(b"Link")
                        + usize::from(cat.get(b"OpenAction").is_some_and(|a| doc.resolve(a).as_dict().is_some()))
                        + usize::from(cat.contains(b"AA"))
                        + pages.iter().filter(|p| p.dict.contains(b"AA")).count()
                        + annots.iter().filter(|a| a.contains(b"AA") || (a.name(b"Subtype") != Some(b"Link") && a.contains(b"A"))).count()
                        + annots.iter().filter(|a| scripted_3d_stream(doc, a).is_some()).count()
                        + sub(doc, &names, b"JavaScript").map_or(0, |t| name_tree_len(doc, &t, 0))
                        + sub(doc, &names, b"AlternatePresentations").map_or(0, |t| name_tree_len(doc, &t, 0))
                }
                // All of it sits under an `ANYWHERE` key.
                Hidden::PrivateData => 0,
            };
            // Plus what the category has under the keys that may sit on any dictionary.
            (h, n.saturating_add(found_anywhere(h)))
        })
        .collect()
}

/// Remove the interactive form: every field is flattened into its page, one without an
/// appearance drawn first so its value stays visible; `/AcroForm` leaves the catalog (`root`);
/// and every widget still on a page goes, undrawn. Returns the objects to retire:
/// the widgets no longer on a page, and each field none of whose widgets is (its /V would
/// otherwise stay reachable).
fn remove_form_fields(doc: &mut Document, root: ObjRef) -> Result<Vec<ObjRef>, RedactError> {
    let fields = pdfcraft_forms::fields(doc);
    // Fields without appearances get one first, so their values stay visible.
    for f in &fields {
        if f.widgets.iter().any(|w| !doc.get(w.obj).as_dict().is_some_and(|d| d.contains(b"AP"))) {
            pdfcraft_forms::redraw_field(doc, &f.name)?;
        }
    }
    let n = pdfcraft_model::pages(doc).len();
    pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), false, true)?;
    doc.update_dict(root, |d| {
        d.remove(b"AcroForm");
    })?;
    // Flattening leaves Hidden and NoView annotations alone (neither drawn nor removed), which
    // is right for Flatten but not here: a widget the user cannot see still holds its field's
    // name and value. Every widget still on a page goes, undrawn; what stays is `on_page`.
    let mut on_page: HashSet<ObjRef> = HashSet::new();
    for p in pdfcraft_model::pages(doc) {
        let list = annots_of(doc, &p.dict);
        let kept: Vec<Object> =
            list.iter().filter(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype")) != Some(b"Widget")).cloned().collect();
        on_page.extend(kept.iter().filter_map(Object::as_ref));
        if kept.len() != list.len() {
            doc.update_dict(p.obj, |d| put_annots(d, kept))?;
        }
    }
    // The widgets taken off the pages are deleted, and so is a field none of whose widgets is on
    // a page any more.
    let mut retired = Vec::new();
    for f in &fields {
        let gone: Vec<ObjRef> = f.widgets.iter().map(|w| w.obj).filter(|w| !on_page.contains(w)).collect();
        if gone.len() == f.widgets.len() {
            retired.push(f.obj);
        }
        retired.extend(gone);
    }
    Ok(retired)
}

/// Remove the chosen categories. Returns what was removed per category.
pub fn remove_hidden(doc: &mut Document, which: &[Hidden]) -> Result<Vec<(Hidden, usize)>, RedactError> {
    let counts = scan(doc);
    let count = |h: Hidden| counts.iter().find(|c| c.0 == h).map_or(0, |c| c.1);
    let (root, _) = catalog(doc).ok_or(RedactError::NothingToApply)?;
    let mut done = Vec::new();
    // Content first (it needs the layers and the form still in place).
    let (text, layers) = (which.contains(&Hidden::HiddenText), which.contains(&Hidden::HiddenLayers));
    if (text && count(Hidden::HiddenText) > 0) || (layers && count(Hidden::HiddenLayers) > 0) {
        content_pass(doc, text, layers, true)?;
    }
    // The annotation, widget and field objects removed: nulled at the end (`retire`), so a
    // structure element, a reply or anything else that still refers to one cannot carry it into
    // the saved file.
    let mut retired: Vec<ObjRef> =
        if which.contains(&Hidden::FormFields) && count(Hidden::FormFields) > 0 { remove_form_fields(doc, root)? } else { Vec::new() };
    // Annotations: comments, attachments (multimedia included), links, actions.
    let drop_annot = |a: &Dict| -> bool {
        let s = a.name(b"Subtype").unwrap_or(b"");
        (which.contains(&Hidden::Comments) && is_comment(s))
            || (which.contains(&Hidden::Attachments) && (s == b"FileAttachment" || is_multimedia(s)))
            || (which.contains(&Hidden::LinksActionsScripts) && s == b"Link")
    };
    let actions = which.contains(&Hidden::LinksActionsScripts);
    // A stored page thumbnail shows the page as it was: it goes whenever a category changes
    // what the page shows (content, annotations or fields).
    let thumbs = which.iter().any(|h| {
        matches!(
            h,
            Hidden::HiddenText | Hidden::HiddenLayers | Hidden::Comments | Hidden::Attachments | Hidden::FormFields | Hidden::LinksActionsScripts
        )
    });
    for p in pdfcraft_model::pages(doc) {
        let list = annots_of(doc, &p.dict);
        let mut removed: Vec<ObjRef> = Vec::new();
        for a in &list {
            if let Some(r) = a.as_ref()
                && doc.get(r).as_dict().is_some_and(drop_annot)
            {
                removed.push(r);
            }
        }
        let mut kept = Vec::new();
        for a in list {
            let Some(r) = a.as_ref() else {
                kept.push(a);
                continue;
            };
            let d = doc.get(r).as_dict().cloned().unwrap_or_default();
            let parent_gone = d.get(b"Parent").and_then(Object::as_ref).is_some_and(|x| removed.contains(&x));
            if removed.contains(&r) || (d.name(b"Subtype") == Some(b"Popup") && parent_gone) {
                retired.push(r);
                continue;
            }
            // Every annotation's actions: a Screen's rendition or script as much as a widget's.
            if actions && (d.contains(b"AA") || d.contains(b"A")) {
                doc.update_dict(r, |d| {
                    d.remove(b"AA");
                    d.remove(b"A");
                })?;
            }
            if actions && let Some(s) = scripted_3d_stream(doc, &d) {
                doc.update_dict(s, |d| {
                    d.remove(b"OnInstantiate");
                })?;
            }
            kept.push(a);
        }
        doc.update_dict(p.obj, |d| {
            put_annots(d, kept);
            if thumbs {
                d.remove(b"Thumb");
            }
            if actions {
                d.remove(b"AA");
            }
        })?;
    }
    let off = hidden_layers(doc);
    let mut cat = doc.get(root).as_dict().cloned().unwrap_or_default();
    let mut names = sub(doc, &cat, b"Names");
    for &h in which {
        match h {
            Hidden::Metadata => {
                doc.trailer_mut().remove(b"Info");
            }
            Hidden::Attachments => {
                if let Some(n) = names.as_mut() {
                    n.remove(b"EmbeddedFiles");
                    n.remove(b"Renditions");
                }
            }
            Hidden::Bookmarks => {
                cat.remove(b"Outlines");
                if cat.name(b"PageMode") == Some(b"UseOutlines") {
                    cat.remove(b"PageMode");
                }
            }
            Hidden::LinksActionsScripts => {
                if cat.get(b"OpenAction").is_some_and(|a| doc.resolve(a).as_dict().is_some()) {
                    cat.remove(b"OpenAction");
                }
                cat.remove(b"AA");
                if let Some(n) = names.as_mut() {
                    n.remove(b"JavaScript");
                    n.remove(b"AlternatePresentations");
                }
            }
            Hidden::HiddenLayers if !off.is_empty() => {
                // The off layers leave the configuration (their content is gone already).
                if let Some(mut oc) = sub(doc, &cat, b"OCProperties") {
                    let keep = |o: &Object| !o.as_ref().is_some_and(|r| off.contains(&r));
                    if let Some(Object::Array(a)) = oc.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
                        oc.set(b"OCGs".to_vec(), Object::Array(a.into_iter().filter(keep).collect()));
                    }
                    if let Some(mut d) = sub(doc, &oc, b"D") {
                        for k in [&b"ON"[..], b"OFF", b"Order", b"Locked"] {
                            if let Some(Object::Array(a)) = d.get(k).map(|g| (*doc.resolve(g)).clone()) {
                                d.set(k.to_vec(), Object::Array(a.into_iter().filter(keep).collect()));
                            }
                        }
                        oc.set(b"D".to_vec(), Object::Dict(d));
                    }
                    cat.set(b"OCProperties".to_vec(), Object::Dict(oc));
                }
            }
            _ => {}
        }
        let n = count(h);
        if n > 0 {
            done.push((h, n));
        }
    }
    if let Some(n) = names {
        cat.set(b"Names".to_vec(), Object::Dict(n));
    }
    doc.set(root, Object::Dict(cat));
    // What was removed is deleted, so no other referrer keeps it in the saved file.
    crate::retire(doc, &retired)?;
    // XMP, associated files and private data may sit on any dictionary, the catalog and pages
    // included: strip them from every object still reachable from the catalog rather than from a
    // fixed list.
    let keys: Vec<&[u8]> = ANYWHERE.into_iter().filter(|(h, ..)| which.contains(h)).map(|(_, key, _)| key).collect();
    if !keys.is_empty() {
        for r in reachable(doc) {
            if let Some(stripped) = strip_keys(&doc.get(r), &keys, 0) {
                doc.set(r, stripped);
            }
        }
    }
    if done.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    doc.require_full_save();
    Ok(done)
}

/// Sanitize Document: remove every category of hidden information.
pub fn sanitize(doc: &mut Document) -> Result<Vec<(Hidden, usize)>, RedactError> {
    match remove_hidden(doc, &HIDDEN) {
        // Nothing hidden is fine for Sanitize: the full rewrite still drops old revisions.
        Err(RedactError::NothingToApply) => {
            doc.require_full_save();
            Ok(Vec::new())
        }
        r => r,
    }
}
