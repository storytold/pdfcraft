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
///
/// [`message`](Issue::message) is the problem in English, ready to read. An interface that reads in
/// another language says it from [`pattern`](Issue::pattern) and [`value`](Issue::value) instead:
/// the pattern is wording of ours, listed in [`WORDINGS`], and the value — when there is one — is
/// the document's own name for something, a PDF keyword that reads the same in every language.
/// [`Issue::said`] does that.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// The ISO 19005-2 clause.
    pub clause: &'static str,
    pub message: String,
    /// The problem as it is worded here, with `{}` where the document's own name goes.
    pub pattern: &'static str,
    /// What goes in the pattern's `{}`: an annotation subtype, an action, a font, a page number.
    pub value: Option<String>,
    /// 0-based page, when the problem is on a page.
    pub page: Option<usize>,
    /// Whether [`convert`] can fix it.
    pub fixable: bool,
}

impl Issue {
    /// The problem as one sentence, said in the language `words` translates into.
    ///
    /// The wording is translated and the document's own name goes in afterwards, in one pass, so a
    /// font called `{}` cannot turn into anything else. With [`in_english`] this is
    /// [`message`](Issue::message).
    pub fn said(&self, words: Words) -> String {
        let said = words(self.pattern);
        match &self.value {
            Some(v) => said.replacen("{}", v, 1),
            None => said,
        }
    }
}

/// How an interface says one of our wordings in its own language: given the English in [`WORDINGS`],
/// the same wording in that language. [`in_english`] keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// The [`Words`] that translate nothing: the wording as it is written here.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

/// Every problem [`verify`] can report, in English, with `{}` where the document's own name goes.
///
/// An interface that shows them in another language translates these, which is the whole reason they
/// are gathered here; `wordings_are_complete` fails if a rule says something that is not on the
/// list. What [`convert`] reports as fixed is not here: that is read by scripts, in English.
pub const WORDINGS: &[&str] = &[
    // Identification and metadata.
    "The document has no XMP metadata",
    "The metadata doesn't identify the file as {}",
    "The title in the metadata and the document information differ",
    // The document as a whole.
    "The document is encrypted",
    "Device colour is used, but there is no PDF/A output intent",
    "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)",
    "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile",
    "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile",
    "The document has additional actions (/AA)",
    "The document has document-level JavaScript",
    "The form asks viewers to make appearances (NeedAppearances)",
    // Pages, actions, annotations and fields.
    "Page {} has additional actions",
    "A {} action isn't allowed",
    "{} annotations aren't allowed",
    "A {} annotation isn't set to print, or is hidden",
    "A {} annotation has no appearance stream",
    "A {} annotation has additional actions",
    "A form field has additional actions",
    // Fonts, streams, images and embedded files.
    "The font {} isn't embedded",
    "A font isn't embedded",
    "A stream uses LZW compression",
    "A stream refers to an external file",
    "An image asks to be interpolated",
    "An image has alternates or OPI information",
    "PostScript XObjects aren't allowed",
    "Embedded files must be PDF/A files themselves (use PDF/A-3 for others)",
    "An embedded file has no AFRelationship",
];

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
        Object::Stream(s) => s.decoded_within(xmp::MAX_XMP_BYTES).ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// What the document declares about its standards.
pub fn declared(doc: &Document) -> Declared {
    let mut d = Declared::default();
    if let Some(x) = metadata(doc) {
        let part = xmp::identifier(&x, xmp::PDFA_ID, "part").and_then(|p| p.parse().ok());
        let conf = xmp::identifier(&x, xmp::PDFA_ID, "conformance").unwrap_or_default();
        d.pdfa = part.map(|p| (p, conf.to_uppercase()));
        d.pdfua = xmp::identifier(&x, xmp::PDFUA_ID, "part").and_then(|p| p.parse().ok());
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
    pdfa_intent_profile(doc).is_some()
}

/// Largest output-intent profile decoded to read its header; real ICC profiles stay well under it.
const MAX_ICC_PROFILE: usize = 16 << 20;

/// The PDF/A output intent's destination profile: `Some(n)` with the profile's number of colour
/// components (`/N`, else the ICC header's data colour space; `None` when neither says), or
/// `None` when the document has no PDF/A output intent.
fn pdfa_intent_profile(doc: &Document) -> Option<Option<i64>> {
    let cat = catalog(doc);
    let list = cat.get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    list.iter().find_map(|oi| {
        let d = doc.resolve(oi).as_dict().cloned()?;
        if d.name(b"S") != Some(b"GTS_PDFA1") {
            return None;
        }
        let profile = doc.resolve(d.get(b"DestOutputProfile")?);
        let Object::Stream(s) = &*profile else { return Some(None) };
        Some(s.dict.int(b"N").or_else(|| {
            // ICC.1 header: the data colour space is bytes 16–19.
            match s.decoded_within(MAX_ICC_PROFILE).ok()?.get(16..20)? {
                b"GRAY" => Some(1),
                b"RGB " => Some(3),
                b"CMYK" => Some(4),
                _ => None,
            }
        }))
    })
}

/// Which device colour spaces the document uses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct DeviceColour {
    gray: bool,
    rgb: bool,
    cmyk: bool,
    /// Resources that define `DefaultRGB` / `DefaultCMYK`: device colour there is drawn in that
    /// space, which PDF/A accepts whatever the output intent (ISO 19005-2 6.2.4.3).
    default_rgb: bool,
    default_cmyk: bool,
}

impl DeviceColour {
    fn any(self) -> bool {
        self.gray || self.rgb || self.cmyk
    }

    /// Note the device colour space `name`, if it is one. Inline images may also use the
    /// abbreviations (`G`, `RGB`, `CMYK`); anywhere else those are resource names.
    fn mark(&mut self, name: &[u8], inline: bool) {
        match name {
            b"DeviceGray" => self.gray = true,
            b"DeviceRGB" => self.rgb = true,
            b"DeviceCMYK" => self.cmyk = true,
            b"G" if inline => self.gray = true,
            b"RGB" if inline => self.rgb = true,
            b"CMYK" if inline => self.cmyk = true,
            _ => {}
        }
    }
}

/// How deep [`type3_procs`] looks into an object's direct dictionaries and arrays.
const MAX_DIRECT_DEPTH: u8 = 8;

/// The glyph procedures of every Type 3 font in `d`: `d` itself, or a font held directly inside
/// it (a font can sit inline in a resource dictionary).
fn type3_procs(doc: &Document, d: &Dict, depth: u8, out: &mut std::collections::HashSet<ObjRef>) {
    if d.name(b"Subtype") == Some(b"Type3")
        && let Some(procs) = d.get(b"CharProcs").map(|c| doc.resolve(c))
        && let Some(procs) = procs.as_dict()
    {
        out.extend(procs.iter().filter_map(|(_, p)| p.as_ref()));
    }
    for (_, v) in d.iter() {
        type3_procs_within(doc, v, depth.saturating_add(1), out);
    }
}

fn type3_procs_within(doc: &Document, o: &Object, depth: u8, out: &mut std::collections::HashSet<ObjRef>) {
    if depth > MAX_DIRECT_DEPTH {
        return;
    }
    match o {
        Object::Dict(d) => type3_procs(doc, d, depth, out),
        Object::Array(a) => {
            for v in a {
                type3_procs_within(doc, v, depth.saturating_add(1), out);
            }
        }
        _ => {}
    }
}

/// The content streams that only a reference identifies, their dictionaries saying nothing: the
/// pages' `/Contents` and Type 3 glyph procedures.
fn referenced_content(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> std::collections::HashSet<ObjRef> {
    let mut out = std::collections::HashSet::new();
    for p in pdfcraft_model::pages(doc) {
        let contents = match p.dict.get(b"Contents") {
            Some(Object::Ref(r)) => match &*doc.get(*r) {
                Object::Array(a) => a.clone(),
                _ => vec![Object::Ref(*r)],
            },
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        out.extend(contents.iter().filter_map(Object::as_ref));
    }
    for (_, o) in objs {
        if let Some(d) = dict_of(o) {
            type3_procs(doc, d, 0, &mut out);
        }
    }
    out
}

/// Whether a stream's own dictionary says it holds content: a form XObject, or a stream with a
/// `/BBox` and no `/Subtype` (a tiling pattern, or an appearance stream that leaves out
/// `/Subtype /Form`). Mesh shadings may carry a `/BBox` too, but their data isn't content.
fn is_content(d: &Dict) -> bool {
    match d.name(b"Subtype") {
        Some(t) => t == b"Form",
        None => d.get(b"BBox").is_some() && d.get(b"ShadingType").is_none(),
    }
}

/// The device colour a content stream sets. It is read operator by operator, so the bytes of a
/// string, a comment or an inline image are never taken for an operator, and an operator right
/// before a delimiter (`1 k/GS0 gs`) is still found.
///
/// Operators are read one at a time and dropped, so a stream of millions of tiny operators never
/// becomes millions of parsed ones in memory, and reading stops once all three spaces are set.
/// Returns how many operators it read.
fn content_colour(data: &[u8], used: &mut DeviceColour) -> usize {
    let mut read = 0;
    for op in pdfcraft_content::ops(data) {
        if used.gray && used.rgb && used.cmyk {
            break;
        }
        read += 1;
        match op.op.as_slice() {
            b"g" | b"G" => used.gray = true,
            b"rg" | b"RG" => used.rgb = true,
            b"k" | b"K" => used.cmyk = true,
            b"cs" | b"CS" => {
                if let Some(name) = op.name(0) {
                    used.mark(name, false);
                }
            }
            b"BI" => {
                let Some((image, _)) = &op.inline else { continue };
                match image.get(b"CS").or_else(|| image.get(b"ColorSpace")) {
                    Some(Object::Name(n)) => used.mark(n, true),
                    // An indexed space draws in its base: `[/I /DeviceRGB 1 <…>]`.
                    Some(Object::Array(a)) if a.first().and_then(Object::as_name).is_some_and(|n| n == b"I" || n == b"Indexed") => {
                        if let Some(base) = a.get(1).and_then(Object::as_name) {
                            used.mark(base, true);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    read
}

/// Whether any content or object uses a device colour space.
fn uses_device_colour(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> DeviceColour {
    let mut used = DeviceColour::default();
    let referenced = referenced_content(doc, objs);
    for (r, o) in objs {
        let Some(d) = dict_of(o) else { continue };
        // A resource dictionary's /ColorSpace, whether it is this object or the page's inline
        // /Resources.
        let resources = d.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        for holder in [Some(d), resources.as_ref()].into_iter().flatten() {
            if let Some(spaces) = holder.get(b"ColorSpace").map(|c| doc.resolve(c))
                && let Some(spaces) = spaces.as_dict()
            {
                used.default_rgb |= spaces.get(b"DefaultRGB").is_some();
                used.default_cmyk |= spaces.get(b"DefaultCMYK").is_some();
            }
        }
        if let Some(cs) = d.get(b"ColorSpace").map(|c| doc.resolve(c)) {
            match &*cs {
                Object::Name(n) if n == b"DeviceGray" => used.gray = true,
                Object::Name(n) if n == b"DeviceRGB" => used.rgb = true,
                Object::Name(n) if n == b"DeviceCMYK" => used.cmyk = true,
                _ => {}
            }
        }
        // Content streams only: a font program or an ICC profile is data, whatever bytes it holds.
        // Parsing is the expensive part, so once content has used all three spaces no more is read.
        if !(used.gray && used.rgb && used.cmyk)
            && let Object::Stream(s) = &**o
            && (referenced.contains(r) || is_content(&s.dict))
            && let Ok(data) = s.decoded()
        {
            let _ = content_colour(&data, &mut used);
        }
    }
    used
}

#[cfg(test)]
thread_local! {
    /// Page `/Annots` entries read while matching annotations to their pages.
    static ANNOT_ENTRIES_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Check `doc` against `level`.
pub fn verify(doc: &Document, level: Level) -> Vec<Issue> {
    let mut out = Vec::new();
    // Each problem is worded here and named by the document there: the wording goes on the list in
    // `WORDINGS` for an interface to translate, and the name goes in its `{}` as it stands.
    let mut issue = |clause: &'static str, pattern: &'static str, value: Option<String>, page: Option<usize>, fixable: bool| {
        let message = match &value {
            Some(v) => pattern.replacen("{}", v, 1),
            None => pattern.to_owned(),
        };
        out.push(Issue { clause, message, pattern, value, page, fixable });
    };
    let cat = catalog(doc);
    // 6.1.3 / 6.6: identification and metadata.
    match metadata(doc) {
        None => issue("6.6.2.1", "The document has no XMP metadata", None, None, true),
        Some(x) => {
            let part = xmp::identifier(&x, xmp::PDFA_ID, "part");
            let conf = xmp::identifier(&x, xmp::PDFA_ID, "conformance");
            if part.as_deref() != Some(&level.part().to_string()) || conf.as_deref().map(str::to_uppercase).as_deref() != Some("B") {
                issue("6.6.4", "The metadata doesn't identify the file as {}", Some(level.label().to_owned()), None, true);
            }
            if let Some(title) = info(doc, "Title")
                && xmp::value(&x, "dc:title").is_some_and(|t| t != title)
            {
                issue("6.6.3", "The title in the metadata and the document information differ", None, None, true);
            }
        }
    }
    // 6.1.3: encryption.
    if doc.trailer().get(b"Encrypt").is_some() {
        issue("6.1.3", "The document is encrypted", None, None, true);
    }
    // 6.2.2: output intent when device colour is used.
    let objs = objects(doc);
    let used = uses_device_colour(doc, &objs);
    match pdfa_intent_profile(doc) {
        None if used.any() => {
            issue("6.2.3", "Device colour is used, but there is no PDF/A output intent", None, None, !used.cmyk);
            if used.cmyk {
                issue("6.2.4.3", "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)", None, None, false);
            }
        }
        // 6.2.4.3: DeviceRGB needs an RGB output intent and DeviceCMYK a CMYK one (#667).
        Some(n) => {
            if used.cmyk && !used.default_cmyk && n != Some(4) {
                issue("6.2.4.3", "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile", None, None, false);
            }
            if used.rgb && !used.default_rgb && n != Some(3) {
                issue("6.2.4.3", "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile", None, None, false);
            }
        }
        None => {}
    }
    // 6.1.3 / 6.6.1: catalog actions.
    if cat.get(b"AA").is_some() {
        issue("6.5.2", "The document has additional actions (/AA)", None, None, true);
    }
    if let Some(names) = cat.get(b"Names").map(|n| doc.resolve(n))
        && names.as_dict().is_some_and(|n| n.get(b"JavaScript").is_some())
    {
        issue("6.5.1", "The document has document-level JavaScript", None, None, true);
    }
    if let Some(af) = cat.get(b"AcroForm").map(|a| doc.resolve(a))
        && af.as_dict().is_some_and(|a| matches!(a.get(b"NeedAppearances"), Some(Object::Bool(true))))
    {
        issue("6.4.1", "The form asks viewers to make appearances (NeedAppearances)", None, None, true);
    }
    // Pages: annotations and page actions.
    let pages = pdfcraft_model::pages(doc);
    // Each annotation's page: the first page that lists it.
    let mut annot_pages: std::collections::HashMap<ObjRef, usize> = std::collections::HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = p.dict.get(b"Annots").map(|a| doc.resolve(a))
            && let Some(a) = a.as_array()
        {
            #[cfg(test)]
            ANNOT_ENTRIES_READ.with(|n| n.set(n.get() + a.len()));
            for r in a.iter().filter_map(Object::as_ref) {
                annot_pages.entry(r).or_insert(i);
            }
        }
    }
    let page_of = |r: ObjRef| annot_pages.get(&r).copied();
    for (i, p) in pages.iter().enumerate() {
        if p.dict.get(b"AA").is_some() {
            issue("6.5.2", "Page {} has additional actions", Some((i + 1).to_string()), Some(i), true);
        }
    }
    check_objects(doc, level, &objs, &page_of, &mut issue);
    // One message per kind of problem is enough for repeated ones (as preflight summaries do).
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert((i.clause, i.message.clone(), i.page)));
    out
}

fn check_objects(
    doc: &Document,
    level: Level,
    objects: &[(ObjRef, std::sync::Arc<Object>)],
    page_of: &impl Fn(ObjRef) -> Option<usize>,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) {
    for (r, o) in objects {
        let Some(d) = dict_of(o) else { continue };
        check_one_object(doc, level, *r, d, o, page_of, issue);
    }
}

fn check_one_object(
    doc: &Document,
    level: Level,
    reference: ObjRef,
    dict: &Dict,
    object: &Object,
    page_of: &impl Fn(ObjRef) -> Option<usize>,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) {
    let ty = dict.name(b"Type");
    let subtype = dict.name(b"Subtype");
    check_actions(doc, dict, issue);
    if is_annotation(dict, ty, subtype) && check_annotation(doc, dict, subtype, page_of(reference), issue) {
        // A forbidden annotation stops every later check for its object, as the scan did before
        // it was split into helpers: the rest of its own checks and the form field, font, stream
        // and embedded file below all came after that `continue`.
        return;
    }
    if dict.get(b"FT").is_some() && dict.get(b"AA").is_some() && ty != Some(b"Annot") {
        issue("6.5.2", "A form field has additional actions", None, None, true);
    }
    if ty == Some(b"Font") || dict.get(b"BaseFont").is_some() && subtype.is_some() {
        check_font(doc, dict, issue);
    }
    if let Object::Stream(stream) = object {
        check_stream(doc, stream, subtype, issue);
    }
    if ty == Some(b"Filespec") && dict.get(b"EF").is_some() {
        check_embedded_file(level, dict, issue);
    }
}

fn check_actions(doc: &Document, dict: &Dict, issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool)) {
    let own = (dict.get(b"S").is_some() && is_action(dict)).then_some(dict);
    let direct = [b"A".as_slice(), b"OpenAction"].map(|key| dict.get(key).and_then(|action| action.as_dict()));
    for action in [own].into_iter().chain(direct).flatten() {
        if let Some(name) = chain_forbidden(doc, action) {
            issue("6.5.1", "A {} action isn't allowed", Some(name), None, true);
        }
    }
}

fn is_annotation(dict: &Dict, ty: Option<&[u8]>, subtype: Option<&[u8]>) -> bool {
    ty == Some(b"Annot") || (subtype.is_some() && dict.get(b"Rect").is_some() && dict.get(b"P").is_some())
}

fn check_annotation(
    doc: &Document,
    dict: &Dict,
    subtype: Option<&[u8]>,
    page: Option<usize>,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) -> bool {
    let name = subtype.map(|bytes| String::from_utf8_lossy(bytes).into_owned()).unwrap_or_default();
    if subtype.is_some_and(|value| FORBIDDEN_ANNOTS.contains(&value)) {
        issue("6.3.1", "{} annotations aren't allowed", Some(name), page, true);
        return true;
    }
    let flags = dict.int(b"F").unwrap_or(0);
    if subtype != Some(b"Popup") && (flags & 4 == 0 || flags & (1 | 2 | 32) != 0) {
        issue("6.3.2", "A {} annotation isn't set to print, or is hidden", Some(name.clone()), page, true);
    }
    check_annotation_appearance(doc, dict, subtype, &name, page, issue);
    if dict.get(b"AA").is_some() {
        issue("6.5.2", "A {} annotation has additional actions", Some(name), page, true);
    }
    false
}

fn check_annotation_appearance(
    doc: &Document,
    dict: &Dict,
    subtype: Option<&[u8]>,
    name: &str,
    page: Option<usize>,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) {
    let has_appearance = dict
        .get(b"AP")
        .map(|appearance| doc.resolve(appearance))
        .and_then(|appearance| appearance.as_dict().cloned())
        .is_some_and(|appearance| appearance.get(b"N").is_some());
    if has_appearance || matches!(subtype, Some(b"Popup") | Some(b"Link")) {
        return;
    }
    let zero_area = dict.get(b"Rect").map(|rect| doc.resolve(rect)).and_then(|rect| rect.as_array().cloned()).is_some_and(|values| {
        let values: Vec<f64> = values.iter().filter_map(|value| doc.resolve(value).as_f64()).collect();
        values.len() == 4 && (values[2] - values[0]).abs() < 1e-6 && (values[3] - values[1]).abs() < 1e-6
    });
    if !zero_area {
        issue("6.3.3", "A {} annotation has no appearance stream", Some(name.to_owned()), page, false);
    }
}

fn check_stream(
    doc: &Document,
    stream: &pdfcraft_cos::Stream,
    subtype: Option<&[u8]>,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) {
    let filters: Vec<Vec<u8>> = match stream.dict.get(b"Filter").map(|filter| doc.resolve(filter)) {
        Some(filter) => match &*filter {
            Object::Name(name) => vec![name.clone()],
            Object::Array(filters) => filters.iter().filter_map(|filter| filter.as_name().map(<[u8]>::to_vec)).collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    if filters.iter().any(|filter| filter == b"LZWDecode") {
        issue("6.1.7.2", "A stream uses LZW compression", None, None, false);
    }
    if stream.dict.get(b"F").is_some() || stream.dict.get(b"FFilter").is_some() {
        issue("6.1.7.1", "A stream refers to an external file", None, None, false);
    }
    if subtype == Some(b"Image") {
        if matches!(stream.dict.get(b"Interpolate"), Some(Object::Bool(true))) {
            issue("6.2.8", "An image asks to be interpolated", None, None, true);
        }
        if stream.dict.get(b"Alternates").is_some() || stream.dict.get(b"OPI").is_some() {
            issue("6.2.8", "An image has alternates or OPI information", None, None, true);
        }
    }
    if subtype == Some(b"PS") {
        issue("6.2.9", "PostScript XObjects aren't allowed", None, None, false);
    }
}

fn check_embedded_file(level: Level, dict: &Dict, issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool)) {
    match level {
        Level::A2b => issue("6.8", "Embedded files must be PDF/A files themselves (use PDF/A-3 for others)", None, None, false),
        Level::A3b if dict.get(b"AFRelationship").is_none() => {
            issue("6.8", "An embedded file has no AFRelationship", None, None, true);
        }
        Level::A3b => {}
    }
}

fn check_font(doc: &Document, d: &Dict, issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool)) {
    let sub = d.name(b"Subtype");
    if sub == Some(b"Type3") {
        return;
    }
    let name = d.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned());
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
        // A font the document doesn't name gets a sentence of its own, rather than the word for
        // "a font" put where a name goes, which read badly even in English.
        match name {
            Some(n) => issue("6.2.11.4", "The font {} isn't embedded", Some(n), None, false),
            None => issue("6.2.11.4", "A font isn't embedded", None, None, false),
        }
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

/// How many actions one action and the `/Next` actions written inside it may hold before the
/// whole chain counts as forbidden, since it can't be checked.
const MAX_ACTION_CHAIN: usize = 1024;

/// The first forbidden action type in the action `a` or in the actions written directly in its
/// `/Next` (ISO 32000-2 12.6.2: a dictionary or an array of them, which may be an object of its
/// own). An action `/Next` refers to is an object of its own, checked and removed as one.
fn chain_forbidden(doc: &Document, a: &Dict) -> Option<String> {
    let mut read = 0;
    let mut seen = std::collections::HashSet::new();
    // Actions written in an indirect `/Next` array, copied out of it.
    let mut held: Vec<Dict> = Vec::new();
    if let Some(s) = walk_chain(doc, a, &mut read, &mut seen, &mut held) {
        return Some(s);
    }
    while let Some(d) = held.pop() {
        if let Some(s) = walk_chain(doc, &d, &mut read, &mut seen, &mut held) {
            return Some(s);
        }
    }
    None
}

/// [`chain_forbidden`] for the actions reachable from `a` through direct `/Next` entries. The
/// actions of each indirect `/Next` array (each read once) go to `held`.
fn walk_chain(doc: &Document, a: &Dict, read: &mut usize, seen: &mut std::collections::HashSet<ObjRef>, held: &mut Vec<Dict>) -> Option<String> {
    let mut stack = vec![a];
    while let Some(d) = stack.pop() {
        *read += 1;
        if *read > MAX_ACTION_CHAIN {
            return Some("too deeply chained".into());
        }
        if let Some(s) = d.name(b"S")
            && FORBIDDEN_ACTIONS.contains(&s)
        {
            return Some(String::from_utf8_lossy(s).into_owned());
        }
        match d.get(b"Next") {
            Some(Object::Dict(next)) => stack.push(next),
            Some(Object::Array(list)) => stack.extend(list.iter().filter_map(Object::as_dict)),
            Some(Object::Ref(r)) if seen.insert(*r) => {
                if let Object::Array(list) = &*doc.get(*r) {
                    // More than the chain limit can't be read anyway.
                    held.extend(list.iter().filter_map(Object::as_dict).take(MAX_ACTION_CHAIN).cloned());
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `o` is (or refers to) a forbidden action or one chaining to it, or to nothing.
fn forbidden(doc: &Document, o: &Object) -> bool {
    match &*doc.resolve(o) {
        Object::Dict(a) => chain_forbidden(doc, a).is_some(),
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
    // An sRGB output intent describes RGB and gray content only: a document with CMYK content
    // needs a CMYK output condition, which only the user can choose (#667).
    if !has_pdfa_intent(doc) && !uses_device_colour(doc, &objects(doc)).cmyk {
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
    // Forbidden actions left as objects of their own, or chained to by one, and those written
    // directly in an /A elsewhere (an outline item's, say).
    for (r, o) in objects(doc) {
        let Object::Dict(d) = &*o else { continue };
        if d.get(b"S").is_some() && is_action(d) && chain_forbidden(doc, d).is_some() {
            doc.set(r, Object::Null);
            actions += 1;
        } else if let Some(Object::Dict(a)) = d.get(b"A")
            && chain_forbidden(doc, a).is_some()
        {
            let mut nd = d.clone();
            nd.remove(b"A");
            doc.set(r, Object::Dict(nd));
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
