//! Remove Hidden Information and Sanitize Document (architecture §11.1, execution plan M8.4).
//!
//! [`scan`] counts what each category would remove; [`remove_hidden`] removes the chosen
//! categories; [`sanitize`] removes all of them. Every removal makes the next save a full
//! rewrite, so the earlier revision (which still holds the data) leaves the file.

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::interp::{Mode, Scope, process};
use crate::{RedactError, Report, annots_of, page_streams};

/// The categories of Acrobat's Remove Hidden Information panel that PdfCraft handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hidden {
    /// Document information (`/Info`) and XMP metadata streams.
    Metadata,
    /// Embedded files and file attachment annotations.
    Attachments,
    /// Comments and markup annotations (with their pop-ups).
    Comments,
    /// Interactive form fields (flattened: their appearance stays as page content).
    FormFields,
    /// Text drawn invisibly (render modes 3 and 7) or wholly off the page.
    HiddenText,
    /// Optional content (layers) that is off: its content, then the layers themselves.
    HiddenLayers,
    Bookmarks,
    /// Links, actions (open action, additional actions) and document JavaScript.
    LinksActionsScripts,
    /// Private data of other applications (`/PieceInfo`).
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

fn is_comment(subtype: &[u8]) -> bool {
    !matches!(subtype, b"Link" | b"Widget" | b"Popup" | b"FileAttachment" | b"Redact")
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
        let mut new_list = list.clone();
        let mut changed = false;
        for (i, new) in out.streams.into_iter().enumerate() {
            let Some(bytes) = new else { continue };
            let mut dict = match &*doc.resolve(&list[i]) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            new_list[i] = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict, &bytes))));
            changed = true;
        }
        if changed || !out.xobjects.is_empty() {
            let mut res = resources;
            if !out.xobjects.is_empty() {
                let mut xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
                for (n, r) in &out.xobjects {
                    xo.set(n.clone(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            doc.update_dict(page.obj, |d| {
                d.set(b"Contents".to_vec(), Object::Array(new_list));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
        }
    }
    Ok(total)
}

/// How many items each category would remove (categories with nothing are included as 0).
pub fn scan(doc: &Document) -> Vec<(Hidden, usize)> {
    let cat = catalog(doc).map(|c| c.1).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let annots: Vec<Dict> = pages.iter().flat_map(|p| annots_of(doc, &p.dict)).filter_map(|a| doc.resolve(&a).as_dict().cloned()).collect();
    let count_sub = |s: &[u8]| annots.iter().filter(|a| a.name(b"Subtype") == Some(s)).count();
    let names = sub(doc, &cat, b"Names").unwrap_or_default();
    let mut doc2 = doc.clone();
    HIDDEN
        .into_iter()
        .map(|h| {
            let n = match h {
                Hidden::Metadata => {
                    let info = doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().map(|d| d.len())).unwrap_or(0);
                    let xmp = usize::from(cat.contains(b"Metadata")) + pages.iter().filter(|p| p.dict.contains(b"Metadata")).count();
                    info + xmp
                }
                Hidden::Attachments => sub(doc, &names, b"EmbeddedFiles").map_or(0, |t| name_tree_len(doc, &t, 0)) + count_sub(b"FileAttachment"),
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
                        + annots.iter().filter(|a| a.contains(b"AA") || (a.name(b"Subtype") == Some(b"Widget") && a.contains(b"A"))).count()
                        + sub(doc, &names, b"JavaScript").map_or(0, |t| name_tree_len(doc, &t, 0))
                }
                Hidden::PrivateData => usize::from(cat.contains(b"PieceInfo")) + pages.iter().filter(|p| p.dict.contains(b"PieceInfo")).count(),
            };
            (h, n)
        })
        .collect()
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
    if which.contains(&Hidden::FormFields) && count(Hidden::FormFields) > 0 {
        // Fields without appearances get one first, so their values stay visible.
        for f in pdfcraft_forms::fields(doc) {
            if f.widgets.iter().any(|w| !doc.get(w.obj).as_dict().is_some_and(|d| d.contains(b"AP"))) {
                pdfcraft_forms::redraw_field(doc, &f.name)?;
            }
        }
        let n = pdfcraft_model::pages(doc).len();
        pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), false, true)?;
        doc.update_dict(root, |d| {
            d.remove(b"AcroForm");
        })?;
    }
    // Annotations: comments, attachments, links, actions.
    let drop_annot = |a: &Dict| -> bool {
        let s = a.name(b"Subtype").unwrap_or(b"");
        (which.contains(&Hidden::Comments) && is_comment(s))
            || (which.contains(&Hidden::Attachments) && s == b"FileAttachment")
            || (which.contains(&Hidden::LinksActionsScripts) && s == b"Link")
    };
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
                continue;
            }
            if which.contains(&Hidden::LinksActionsScripts) && (d.contains(b"AA") || (d.name(b"Subtype") == Some(b"Widget") && d.contains(b"A"))) {
                doc.update_dict(r, |d| {
                    d.remove(b"AA");
                    d.remove(b"A");
                })?;
            }
            kept.push(a);
        }
        let private = which.contains(&Hidden::PrivateData);
        let metadata = which.contains(&Hidden::Metadata);
        let actions = which.contains(&Hidden::LinksActionsScripts);
        doc.update_dict(p.obj, |d| {
            if kept.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
            if private {
                d.remove(b"PieceInfo");
            }
            if metadata {
                d.remove(b"Metadata");
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
                cat.remove(b"Metadata");
                doc.trailer_mut().remove(b"Info");
            }
            Hidden::Attachments => {
                if let Some(n) = names.as_mut() {
                    n.remove(b"EmbeddedFiles");
                }
                cat.remove(b"AF");
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
                }
            }
            Hidden::PrivateData => {
                cat.remove(b"PieceInfo");
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
