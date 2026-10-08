//! pdfcraft-preflight — Standards: PDF/A (L4).
//!
//! [`verify`] checks a document against the parts of ISO 19005-2/-3 (PDF/A-2b, PDF/A-3b) that can
//! be decided from the object graph:
//!
//! - identification: XMP metadata with `pdfaid:part` and `pdfaid:conformance`, consistent with
//!   the document information dictionary;
//! - no encryption; no LZW; no external streams;
//! - an output intent (`GTS_PDFA1`) with an ICC profile when device colour is used;
//! - every font embedded;
//! - annotations: allowed types, printable, not hidden, with appearance streams;
//! - actions: none of the forbidden kinds (JavaScript, Launch, Sound, Movie, ResetForm,
//!   ImportData, Hide, SetOCGState, Rendition, Trans, GoTo3DView); no additional actions on the
//!   catalog, pages, fields or widgets; `NeedAppearances` not true;
//! - images: no `Interpolate`, no `Alternates`, no OPI; no PostScript XObjects;
//! - embedded files: PDF/A-2 only allows embedded PDF/A files; PDF/A-3 needs `AFRelationship`.
//!
//! [`convert`] fixes what can be fixed without changing how pages look (XMP, the sRGB output
//! intent, forbidden actions, annotation flags, Interpolate, NeedAppearances, encryption) and
//! returns what remains. Fonts that aren't embedded can't be fixed here.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod icc;
mod xmp;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

/// The PDF/A part and conformance level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    A2b,
    A3b,
}

impl Level {
    pub fn part(self) -> u8 {
        match self {
            Level::A2b => 2,
            Level::A3b => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::A2b => "PDF/A-2b",
            Level::A3b => "PDF/A-3b",
        }
    }

    pub fn from_id(id: &str) -> Option<Level> {
        match id.to_ascii_lowercase().replace(['/', '-', ' '], "").as_str() {
            "pdfa2b" | "a2b" | "2b" => Some(Level::A2b),
            "pdfa3b" | "a3b" | "3b" => Some(Level::A3b),
            _ => None,
        }
    }
}

/// A rule a document breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// The ISO 19005-2 clause.
    pub clause: &'static str,
    pub message: String,
    /// 0-based page, when the problem is on a page.
    pub page: Option<usize>,
    /// Whether [`convert`] can fix it.
    pub fixable: bool,
}

/// What a document declares (the Standards panel).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declared {
    /// `pdfaid:part` and `pdfaid:conformance`, e.g. (2, "B").
    pub pdfa: Option<(u8, String)>,
    pub pdfua: Option<u8>,
    /// Output intents' condition identifiers.
    pub output_intents: Vec<String>,
}

const FORBIDDEN_ACTIONS: &[&[u8]] =
    &[b"JavaScript", b"Launch", b"Sound", b"Movie", b"ResetForm", b"ImportData", b"Hide", b"SetOCGState", b"Rendition", b"Trans", b"GoTo3DView"];
const FORBIDDEN_ANNOTS: &[&[u8]] = &[b"Sound", b"Movie", b"Screen", b"3D", b"RichMedia"];

fn catalog(doc: &Document) -> Dict {
    doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap_or_default()
}

/// The XMP metadata stream of the catalog, as text.
fn metadata(doc: &Document) -> Option<String> {
    let cat = catalog(doc);
    let m = doc.resolve(cat.get(b"Metadata")?);
    match &*m {
        Object::Stream(s) => s.decoded().ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// What the document declares about its standards.
pub fn declared(doc: &Document) -> Declared {
    let mut d = Declared::default();
    if let Some(x) = metadata(doc) {
        let part = xmp::value(&x, "pdfaid:part").and_then(|p| p.parse().ok());
        let conf = xmp::value(&x, "pdfaid:conformance").unwrap_or_default();
        d.pdfa = part.map(|p| (p, conf.to_uppercase()));
        d.pdfua = xmp::value(&x, "pdfuaid:part").and_then(|p| p.parse().ok());
    }
    let cat = catalog(doc);
    if let Some(list) = cat.get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()) {
        for oi in list {
            if let Some(od) = doc.resolve(&oi).as_dict() {
                let id = od.get(b"OutputConditionIdentifier").and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).unwrap_or_default();
                d.output_intents.push(id);
            }
        }
    }
    d
}

/// Every indirect object, with its reference.
fn objects(doc: &Document) -> Vec<(ObjRef, std::sync::Arc<Object>)> {
    doc.object_numbers()
        .into_iter()
        .map(|n| {
            let r = ObjRef::new(n, doc.generation(n));
            (r, doc.get(r))
        })
        .collect()
}

fn dict_of(o: &Object) -> Option<&Dict> {
    match o {
        Object::Dict(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn has_pdfa_intent(doc: &Document) -> bool {
    let cat = catalog(doc);
    cat.get(b"OutputIntents")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .any(|oi| doc.resolve(oi).as_dict().is_some_and(|d| d.name(b"S") == Some(b"GTS_PDFA1") && d.get(b"DestOutputProfile").is_some()))
}

/// Whether any content or object uses a device colour space.
fn uses_device_colour(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> (bool, bool) {
    let (mut rgb_or_gray, mut cmyk) = (false, false);
    for (_, o) in objs {
        let Some(d) = dict_of(o) else { continue };
        if let Some(cs) = d.get(b"ColorSpace").map(|c| doc.resolve(c)) {
            match &*cs {
                Object::Name(n) if n == b"DeviceRGB" || n == b"DeviceGray" => rgb_or_gray = true,
                Object::Name(n) if n == b"DeviceCMYK" => cmyk = true,
                _ => {}
            }
        }
        if let Object::Stream(s) = &**o
            && d.get(b"Type").is_none()
            && d.get(b"Subtype").is_none_or(|t| t.as_name() == Some(b"Form"))
            && let Ok(data) = s.decoded()
        {
            // Content streams: rg/RG/g/G/k/K operators.
            for line in data.split(|b| *b == b'\n' || *b == b' ') {
                match line {
                    b"rg" | b"RG" | b"g" | b"G" => rgb_or_gray = true,
                    b"k" | b"K" => cmyk = true,
                    _ => {}
                }
            }
        }
    }
    (rgb_or_gray, cmyk)
}

/// Check `doc` against `level`.
pub fn verify(doc: &Document, level: Level) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut issue = |clause: &'static str, message: String, page: Option<usize>, fixable: bool| out.push(Issue { clause, message, page, fixable });
    let cat = catalog(doc);
    // 6.1.3 / 6.6: identification and metadata.
    match metadata(doc) {
        None => issue("6.6.2.1", "The document has no XMP metadata".into(), None, true),
        Some(x) => {
            let part = xmp::value(&x, "pdfaid:part");
            let conf = xmp::value(&x, "pdfaid:conformance");
            if part.as_deref() != Some(&level.part().to_string()) || conf.as_deref().map(str::to_uppercase).as_deref() != Some("B") {
                issue("6.6.4", format!("The metadata doesn't identify the file as {}", level.label()), None, true);
            }
            if let Some(title) = info(doc, "Title")
                && xmp::value(&x, "dc:title").is_some_and(|t| t != title)
            {
                issue("6.6.3", "The title in the metadata and the document information differ".into(), None, true);
            }
        }
    }
    // 6.1.3: encryption.
    if doc.trailer().get(b"Encrypt").is_some() {
        issue("6.1.3", "The document is encrypted".into(), None, true);
    }
    // 6.2.2: output intent when device colour is used.
    let objs = objects(doc);
    let (rgb, cmyk) = uses_device_colour(doc, &objs);
    if (rgb || cmyk) && !has_pdfa_intent(doc) {
        issue("6.2.3", "Device colour is used, but there is no PDF/A output intent".into(), None, !cmyk);
        if cmyk {
            issue("6.2.4.3", "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)".into(), None, false);
        }
    }
    // 6.1.3 / 6.6.1: catalog actions.
    if cat.get(b"AA").is_some() {
        issue("6.5.2", "The document has additional actions (/AA)".into(), None, true);
    }
    if let Some(names) = cat.get(b"Names").map(|n| doc.resolve(n))
        && names.as_dict().is_some_and(|n| n.get(b"JavaScript").is_some())
    {
        issue("6.5.1", "The document has document-level JavaScript".into(), None, true);
    }
    if let Some(af) = cat.get(b"AcroForm").map(|a| doc.resolve(a))
        && af.as_dict().is_some_and(|a| matches!(a.get(b"NeedAppearances"), Some(Object::Bool(true))))
    {
        issue("6.4.1", "The form asks viewers to make appearances (NeedAppearances)".into(), None, true);
    }
    // Pages: annotations and page actions.
    let pages = pdfcraft_model::pages(doc);
    let page_of = |r: ObjRef| {
        pages.iter().position(|p| {
            p.dict
                .get(b"Annots")
                .map(|a| doc.resolve(a))
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .any(|x| x.as_ref() == Some(r))
        })
    };
    for (i, p) in pages.iter().enumerate() {
        if p.dict.get(b"AA").is_some() {
            issue("6.5.2", format!("Page {} has additional actions", i + 1), Some(i), true);
        }
    }
    for (r, o) in &objs {
        let Some(d) = dict_of(o) else { continue };
        let ty = d.name(b"Type");
        let sub = d.name(b"Subtype");
        // Actions anywhere (structure elements also have /S: their role).
        if let Some(s) = d.name(b"S")
            && FORBIDDEN_ACTIONS.contains(&s)
            && is_action(d)
        {
            issue("6.5.1", format!("A {} action isn't allowed", String::from_utf8_lossy(s)), None, true);
        }
        // Annotations.
        if ty == Some(b"Annot") || (sub.is_some() && d.get(b"Rect").is_some() && d.get(b"P").is_some()) {
            let page = page_of(*r);
            let name = sub.map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
            if sub.is_some_and(|s| FORBIDDEN_ANNOTS.contains(&s)) {
                issue("6.3.1", format!("{name} annotations aren't allowed"), page, true);
                continue;
            }
            let f = d.int(b"F").unwrap_or(0);
            if sub != Some(b"Popup") && (f & 4 == 0 || f & (1 | 2 | 32) != 0) {
                issue("6.3.2", format!("A {name} annotation isn't set to print, or is hidden"), page, true);
            }
            let has_ap = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).is_some_and(|a| a.get(b"N").is_some());
            if !has_ap && !matches!(sub, Some(b"Popup") | Some(b"Link")) {
                let zero = d.get(b"Rect").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned()).is_some_and(|a| {
                    let v: Vec<f64> = a.iter().filter_map(|n| doc.resolve(n).as_f64()).collect();
                    v.len() == 4 && (v[2] - v[0]).abs() < 1e-6 && (v[3] - v[1]).abs() < 1e-6
                });
                if !zero {
                    issue("6.3.3", format!("A {name} annotation has no appearance stream"), page, false);
                }
            }
            if d.get(b"AA").is_some() {
                issue("6.5.2", format!("A {name} annotation has additional actions"), page, true);
            }
        }
        // Form fields with additional actions (non-widget fields).
        if d.get(b"FT").is_some() && d.get(b"AA").is_some() && ty != Some(b"Annot") {
            issue("6.5.2", "A form field has additional actions".into(), None, true);
        }
        // Fonts.
        if ty == Some(b"Font") || d.get(b"BaseFont").is_some() && sub.is_some() {
            check_font(doc, d, &mut issue);
        }
        // Streams.
        if let Object::Stream(s) = &**o {
            let filters: Vec<Vec<u8>> = match s.dict.get(b"Filter").map(|f| doc.resolve(f)) {
                Some(f) => match &*f {
                    Object::Name(n) => vec![n.clone()],
                    Object::Array(a) => a.iter().filter_map(|x| x.as_name().map(<[u8]>::to_vec)).collect(),
                    _ => Vec::new(),
                },
                None => Vec::new(),
            };
            if filters.iter().any(|f| f == b"LZWDecode") {
                issue("6.1.7.2", "A stream uses LZW compression".into(), None, false);
            }
            if s.dict.get(b"F").is_some() || s.dict.get(b"FFilter").is_some() {
                issue("6.1.7.1", "A stream refers to an external file".into(), None, false);
            }
            if sub == Some(b"Image") {
                if matches!(s.dict.get(b"Interpolate"), Some(Object::Bool(true))) {
                    issue("6.2.8", "An image asks to be interpolated".into(), None, true);
                }
                if s.dict.get(b"Alternates").is_some() || s.dict.get(b"OPI").is_some() {
                    issue("6.2.8", "An image has alternates or OPI information".into(), None, true);
                }
            }
            if sub == Some(b"PS") {
                issue("6.2.9", "PostScript XObjects aren't allowed".into(), None, false);
            }
        }
        // Embedded files.
        if ty == Some(b"Filespec") && d.get(b"EF").is_some() {
            match level {
                Level::A2b => issue("6.8", "Embedded files must be PDF/A files themselves (use PDF/A-3 for others)".into(), None, false),
                Level::A3b => {
                    if d.get(b"AFRelationship").is_none() {
                        issue("6.8", "An embedded file has no AFRelationship".into(), None, true);
                    }
                }
            }
        }
    }
    // One message per kind of problem is enough for repeated ones (as preflight summaries do).
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert((i.clause, i.message.clone(), i.page)));
    out
}

fn check_font(doc: &Document, d: &Dict, issue: &mut impl FnMut(&'static str, String, Option<usize>, bool)) {
    let sub = d.name(b"Subtype");
    if sub == Some(b"Type3") {
        return;
    }
    let name = d.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| "a font".into());
    let descriptor = if sub == Some(b"Type0") {
        d.get(b"DescendantFonts")
            .map(|x| doc.resolve(x))
            .and_then(|a| a.as_array().and_then(|a| a.first().cloned()))
            .map(|f| doc.resolve(&f))
            .and_then(|f| f.as_dict().and_then(|fd| fd.get(b"FontDescriptor").cloned()))
    } else {
        d.get(b"FontDescriptor").cloned()
    };
    let embedded = descriptor
        .map(|fd| doc.resolve(&fd))
        .and_then(|fd| fd.as_dict().cloned())
        .is_some_and(|fd| fd.get(b"FontFile").is_some() || fd.get(b"FontFile2").is_some() || fd.get(b"FontFile3").is_some());
    if !embedded {
        issue("6.2.11.4", format!("The font {name} isn't embedded"), None, false);
    }
}

fn info(doc: &Document, key: &str) -> Option<String> {
    let i = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    i.as_dict()?.get(key.as_bytes()).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text()))
}

/// What [`convert`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub fixed: Vec<String>,
    /// Problems left (the document isn't conforming while any remain).
    pub remaining: Vec<Issue>,
}

/// Whether a dictionary with an `/S` entry is an action (not a structure element or the like).
fn is_action(d: &Dict) -> bool {
    match d.name(b"Type") {
        Some(t) => t == b"Action",
        None => !(d.get(b"P").is_some() && d.get(b"K").is_some()),
    }
}

/// Whether `o` is (or refers to) a forbidden action, or to nothing.
fn forbidden(doc: &Document, o: &Object) -> bool {
    match &*doc.resolve(o) {
        Object::Dict(a) => a.name(b"S").is_some_and(|s| FORBIDDEN_ACTIONS.contains(&s)),
        Object::Null => true,
        _ => false,
    }
}

fn strip_actions(doc: &Document, d: &mut Dict) -> bool {
    let mut changed = d.remove(b"AA").is_some();
    if d.get(b"A").is_some_and(|a| forbidden(doc, a)) {
        d.remove(b"A");
        changed = true;
    }
    changed
}

/// Make `doc` conform to `level` as far as possible, then re-check it.
pub fn convert(doc: &mut Document, level: Level) -> Result<Report, pdfcraft_cos::CosError> {
    let mut fixed = Vec::new();
    if doc.trailer().get(b"Encrypt").is_some() {
        doc.remove_encryption();
        fixed.push("Removed encryption".into());
    }
    let root = doc.root().ok_or(pdfcraft_cos::CosError::Syntax { offset: 0, detail: "no catalog".into() })?;
    // Catalog: actions, JavaScript, NeedAppearances, output intent, metadata.
    let mut cat = catalog(doc);
    if cat.remove(b"AA").is_some() {
        fixed.push("Removed the document's additional actions".into());
    }
    if let Some(oa) = cat.get(b"OpenAction").cloned()
        && matches!(&*doc.resolve(&oa), Object::Dict(_))
        && forbidden(doc, &oa)
    {
        cat.remove(b"OpenAction");
        fixed.push("Removed the open action".into());
    }
    if let Some(mut names) = cat.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_dict().cloned())
        && names.remove(b"JavaScript").is_some()
    {
        cat.set(b"Names".to_vec(), Object::Dict(names));
        fixed.push("Removed document-level JavaScript".into());
    }
    if let Some(Object::Ref(af)) = cat.get(b"AcroForm").cloned() {
        let mut changed = false;
        doc.update_dict(af, |a| changed = a.remove(b"NeedAppearances").is_some())?;
        if changed {
            fixed.push("Cleared NeedAppearances".into());
        }
    } else if let Some(Object::Dict(mut af)) = cat.get(b"AcroForm").cloned()
        && af.remove(b"NeedAppearances").is_some()
    {
        cat.set(b"AcroForm".to_vec(), Object::Dict(af));
        fixed.push("Cleared NeedAppearances".into());
    }
    if !has_pdfa_intent(doc) {
        let mut icc = Dict::new();
        icc.set(b"N".to_vec(), Object::Int(3));
        let profile = doc.add(Object::Stream(Stream::flate(icc, &icc::srgb())));
        let mut oi = Dict::new();
        oi.set(b"Type".to_vec(), Object::name("OutputIntent"));
        oi.set(b"S".to_vec(), Object::name("GTS_PDFA1"));
        oi.set(b"OutputConditionIdentifier".to_vec(), PdfString::text("sRGB IEC61966-2.1"));
        oi.set(b"Info".to_vec(), PdfString::text("sRGB IEC61966-2.1"));
        oi.set(b"DestOutputProfile".to_vec(), Object::Ref(profile));
        let mut list = cat.get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        list.push(Object::Dict(oi));
        cat.set(b"OutputIntents".to_vec(), Object::Array(list));
        fixed.push("Added an sRGB output intent".into());
    }
    let x = xmp::packet(doc, level);
    let mut md = Dict::new();
    md.set(b"Type".to_vec(), Object::name("Metadata"));
    md.set(b"Subtype".to_vec(), Object::name("XML"));
    // Metadata stays uncompressed so other tools can find it.
    let m = doc.add(Object::Stream(Stream::from_raw(md, x.into_bytes())));
    cat.set(b"Metadata".to_vec(), Object::Ref(m));
    fixed.push(format!("Identified the file as {} in its XMP metadata", level.label()));
    doc.update_dict(root, |c| *c = cat)?;

    // Pages, annotations, fields, images, embedded files.
    let (mut actions, mut flags, mut images, mut removed, mut files) = (0, 0, 0, 0, 0);
    for p in pdfcraft_model::pages(doc) {
        let mut changed = false;
        doc.update_dict(p.obj, |d| changed = d.remove(b"AA").is_some())?;
        actions += changed as usize;
    }
    for (r, o) in objects(doc) {
        let Some(d) = dict_of(&o).cloned() else { continue };
        let ty = d.name(b"Type").map(<[u8]>::to_vec);
        let sub = d.name(b"Subtype").map(<[u8]>::to_vec);
        let is_annot = ty.as_deref() == Some(b"Annot");
        if is_annot && sub.as_deref().is_some_and(|s| FORBIDDEN_ANNOTS.contains(&s)) {
            // Take it off its page.
            for p in pdfcraft_model::pages(doc) {
                let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
                if annots.iter().any(|a| a.as_ref() == Some(r)) {
                    let kept: Vec<Object> = annots.into_iter().filter(|a| a.as_ref() != Some(r)).collect();
                    doc.update_dict(p.obj, |pd| pd.set(b"Annots".to_vec(), Object::Array(kept)))?;
                }
            }
            removed += 1;
            continue;
        }
        if matches!(&*o, Object::Dict(_)) && (is_annot || d.get(b"FT").is_some()) {
            let mut nd = d.clone();
            let mut changed = strip_actions(doc, &mut nd);
            actions += changed as usize;
            if is_annot && sub.as_deref() != Some(b"Popup") {
                let f = nd.int(b"F").unwrap_or(0);
                let want = (f | 4) & !(1 | 2 | 32);
                if want != f {
                    nd.set(b"F".to_vec(), Object::Int(want));
                    flags += 1;
                    changed = true;
                }
            }
            if changed {
                doc.set(r, Object::Dict(nd));
            }
            continue;
        }
        if let Object::Stream(s) = &*o
            && sub.as_deref() == Some(b"Image")
        {
            let mut ns = s.clone();
            let mut changed = false;
            if matches!(ns.dict.get(b"Interpolate"), Some(Object::Bool(true))) {
                ns.dict.set(b"Interpolate".to_vec(), Object::Bool(false));
                changed = true;
            }
            changed |= ns.dict.remove(b"Alternates").is_some();
            changed |= ns.dict.remove(b"OPI").is_some();
            if changed {
                doc.set(r, Object::Stream(ns));
                images += 1;
            }
            continue;
        }
        if level == Level::A3b && ty.as_deref() == Some(b"Filespec") && d.get(b"EF").is_some() && d.get(b"AFRelationship").is_none() {
            let mut nd = d.clone();
            nd.set(b"AFRelationship".to_vec(), Object::name("Unspecified"));
            doc.set(r, Object::Dict(nd));
            files += 1;
        }
    }
    // Forbidden actions left as objects of their own (nothing refers to them any more).
    for (r, o) in objects(doc) {
        if let Object::Dict(d) = &*o
            && d.name(b"S").is_some_and(|s| FORBIDDEN_ACTIONS.contains(&s))
            && is_action(d)
        {
            doc.set(r, Object::Null);
            actions += 1;
        }
    }
    for (n, what) in [
        (actions, "removed forbidden or additional actions"),
        (flags, "set annotations to print and show"),
        (images, "cleared image interpolation and alternates"),
        (removed, "removed multimedia annotations"),
        (files, "marked embedded files' relationship"),
    ] {
        if n > 0 {
            fixed.push(format!("{n}: {what}"));
        }
    }
    let remaining = verify(doc, level);
    Ok(Report { fixed, remaining })
}

#[cfg(test)]
mod tests;
