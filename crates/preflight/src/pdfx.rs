//! PDF/X (ISO 15930): verify PDF/X-1a:2001, PDF/X-1a:2003, PDF/X-3:2002, PDF/X-3:2003 and
//! PDF/X-4 from the object graph.
//!
//! The ISO text is not public. The rules come from the CGATS *Application Notes for PDF/X
//! Standards* (version 4, September 2006), the free companion to ISO 15930-1, -3, -4 and -6, and
//! from ISO 32000 for the keys themselves; the PDF/X-4 metadata rules come from public
//! descriptions of ISO 15930-7. Each [`Finding`] names the section of the notes its rule rests on
//! (its [`clause`](Finding::clause)), not an ISO clause number.
//!
//! What [`verify`] checks:
//! - identification: `GTS_PDFXVersion` (and `GTS_PDFXConformance` for PDF/X-1a:2001) in the
//!   document information, or for PDF/X-4 `pdfxid:GTS_PDFXVersion` and the rest of the XMP
//!   metadata; the trailer's document ID; `Trapped` set to `/True` or `/False`;
//! - one `GTS_PDFX` output intent, with an output (printer) ICC profile of a colour space the
//!   level allows, or (PDF/X-1a and -3) a characterization in the ICC registry;
//! - page boxes: a TrimBox or an ArtBox but not both, inside the BleedBox, the CropBox and the
//!   MediaBox, and the BleedBox inside the MediaBox;
//! - annotations outside the BleedBox, and printer marks outside the TrimBox or ArtBox;
//! - every font embedded; no encryption; no LZW (nor JBIG2 before PDF/X-4); no external streams,
//!   reference XObjects, OPI or PostScript XObjects; no JavaScript or additional actions;
//! - transparency (soft masks, opacity below 1, blend modes other than Normal) only in PDF/X-4;
//! - colour: CMYK, gray and spot colour only in PDF/X-1a; in PDF/X-3 and -4, device colour that
//!   matches the output intent (or is mapped by a `Default` space) and device-independent colour
//!   only under an embedded output profile.
//!
//! Device colour that content sets with operators is found by the PDF/A walk
//! ([`uses_device_colour`](super::uses_device_colour)); the colour spaces that images, shadings
//! (including those of shading patterns) and resource dictionaries define, with the bases and
//! alternates under them, are read here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{Words, xmp};

/// A PDF/X conformance level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// ISO 15930-1:2001.
    X1a2001,
    /// ISO 15930-4:2003.
    X1a2003,
    /// ISO 15930-3:2002.
    X3_2002,
    /// ISO 15930-6:2003.
    X3_2003,
    /// ISO 15930-7.
    X4,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::X1a2001, Level::X1a2003, Level::X3_2002, Level::X3_2003, Level::X4];

    pub fn label(self) -> &'static str {
        match self {
            Level::X1a2001 => "PDF/X-1a:2001",
            Level::X1a2003 => "PDF/X-1a:2003",
            Level::X3_2002 => "PDF/X-3:2002",
            Level::X3_2003 => "PDF/X-3:2003",
            Level::X4 => "PDF/X-4",
        }
    }

    /// The part of ISO 15930 that defines the level.
    pub fn standard(self) -> &'static str {
        match self {
            Level::X1a2001 => "ISO 15930-1:2001",
            Level::X1a2003 => "ISO 15930-4:2003",
            Level::X3_2002 => "ISO 15930-3:2002",
            Level::X3_2003 => "ISO 15930-6:2003",
            Level::X4 => "ISO 15930-7",
        }
    }

    /// A level from a short id (`x1a2001`, `x1a` for 2003, `x3`, `x4`) or its label.
    pub fn from_id(id: &str) -> Option<Level> {
        match id.to_ascii_lowercase().replace(['/', '-', ':', ' '], "").trim_start_matches("pdf") {
            "x1a2001" => Some(Level::X1a2001),
            "x1a2003" | "x1a" => Some(Level::X1a2003),
            "x32002" => Some(Level::X3_2002),
            "x32003" | "x3" => Some(Level::X3_2003),
            "x4" => Some(Level::X4),
            _ => None,
        }
    }

    /// The level a `GTS_PDFXVersion` value names.
    pub fn from_version(version: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.version_key() == version)
    }

    /// What `GTS_PDFXVersion` says for this level (CGATS notes 2.3; PDF/X-1a:2001 is a
    /// conformance level of PDF/X-1:2001).
    fn version_key(self) -> &'static str {
        match self {
            Level::X1a2001 => "PDF/X-1:2001",
            Level::X1a2003 => "PDF/X-1a:2003",
            Level::X3_2002 => "PDF/X-3:2002",
            Level::X3_2003 => "PDF/X-3:2003",
            Level::X4 => "PDF/X-4",
        }
    }

    /// PDF/X-1a carries CMYK and spot colour only (CGATS notes table 1, 3.3).
    fn cmyk_only(self) -> bool {
        matches!(self, Level::X1a2001 | Level::X1a2003)
    }

    /// PDF/X-4 is the first level that allows transparency and JBIG2 (CGATS notes 2.8, 2.25).
    fn is_x4(self) -> bool {
        self == Level::X4
    }
}

/// A rule a document breaks.
///
/// As with PDF/A's [`Issue`](crate::Issue), [`message`](Finding::message) is the English sentence,
/// and an interface in another language says it from [`pattern`](Finding::pattern), listed in
/// [`WORDINGS`], with [`value`](Finding::value) — the document's own name for something — put in.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// A stable id for the rule, such as `pdfx.page-boxes`.
    pub rule: &'static str,
    /// Where the rule is described: a section of the CGATS PDF/X application notes, or the part
    /// of ISO 15930 when the notes don't cover it.
    pub clause: &'static str,
    pub message: String,
    pub pattern: &'static str,
    pub value: Option<String>,
    /// 0-based page, when the problem is on a page.
    pub page: Option<usize>,
    /// The object the problem was found in. Repeated problems are reported once, with the first
    /// object that has them.
    pub object: Option<u32>,
}

impl Finding {
    /// The problem as one sentence, said in the language `words` translates into.
    pub fn said(&self, words: Words) -> String {
        let said = words(self.pattern);
        match &self.value {
            Some(v) => said.replacen("{}", v, 1),
            None => said,
        }
    }
}

/// Every problem [`verify`] can report, in English, with `{}` where the document's own name goes.
/// Some are PDF/A's wording for the same problem, and are translated already.
pub const WORDINGS: &[&str] = &[
    // Identification, metadata and trapping.
    "The document information doesn't identify the file as {}",
    "The metadata doesn't identify the file as {}",
    "The document has no XMP metadata",
    "The metadata has no {}",
    "The document information and the metadata name different PDF/X versions",
    "The file trailer has no document ID",
    "The document doesn't say whether it is trapped (Trapped)",
    "Trapped is {}, but it must be /True or /False",
    "The Trapped key and the metadata's pdf:Trapped disagree",
    "The document is encrypted",
    // The output intent.
    "There is no PDF/X output intent (GTS_PDFX)",
    "There is more than one PDF/X output intent",
    "The PDF/X output intent has no OutputConditionIdentifier",
    "The PDF/X output intent has no embedded profile (DestOutputProfile)",
    "The PDF/X output intent has neither an embedded profile nor a characterization in the ICC registry",
    "The output intent's profile isn't a valid ICC profile",
    "The output intent's profile isn't an output (printer) profile",
    "The output intent's profile describes {} colour, but PDF/X-1a needs a CMYK printing condition",
    "The output intent's profile describes {} colour; it must be gray, RGB or CMYK",
    "The output intent's profile has /N {}, which doesn't match its colour space",
    "The output intent's profile has no device-to-PCS transform (AToB1 or grayTRC)",
    // Pages and annotations.
    "The page has no valid MediaBox",
    "The page has neither a TrimBox nor an ArtBox",
    "The page has both a TrimBox and an ArtBox",
    "The {} extends beyond the BleedBox",
    "The {} extends beyond the CropBox",
    "The {} extends beyond the MediaBox",
    "A {} annotation lies within the BleedBox",
    "A PrinterMark annotation overlaps the TrimBox or ArtBox",
    // Fonts, streams and actions.
    "The font {} isn't embedded",
    "A font isn't embedded",
    "A stream uses LZW compression",
    "A stream uses JBIG2 compression",
    "A stream refers to an external file",
    "A form XObject refers to a page of another file (reference XObject)",
    "An image or form has OPI information",
    "PostScript XObjects aren't allowed",
    "The document has document-level JavaScript",
    "A {} action isn't allowed",
    "The document has additional actions (/AA)",
    "Page {} has additional actions",
    "A {} annotation has additional actions",
    "A form field has additional actions",
    // Transparency and colour.
    "A graphics state uses a soft mask",
    "A graphics state sets an opacity below 1",
    "A graphics state uses the {} blend mode",
    "An image has a soft mask",
    "The {} colour space isn't allowed: PDF/X-1a allows only CMYK, gray and spot colour",
    "Content uses DeviceRGB colour: PDF/X-1a allows only CMYK, gray and spot colour",
    "{} colour doesn't match the output intent's colour space",
    "Content uses {} colour, which doesn't match the output intent's colour space",
    "Device-independent colour is used, so the output intent needs an embedded profile",
];

/// A rule and where it is described.
#[derive(Clone, Copy)]
struct Rule {
    id: &'static str,
    clause: &'static str,
}

const IDENTIFICATION: Rule = Rule { id: "pdfx.identification", clause: "CGATS notes 2.3" };
const METADATA: Rule = Rule { id: "pdfx.metadata", clause: "ISO 15930-7 (metadata)" };
const TRAILER_ID: Rule = Rule { id: "pdfx.trailer-id", clause: "CGATS notes 2.22" };
const TRAPPED: Rule = Rule { id: "pdfx.trapped", clause: "CGATS notes 2.17" };
const ENCRYPTION: Rule = Rule { id: "pdfx.encryption", clause: "CGATS notes 2.11" };
const OUTPUT_INTENT: Rule = Rule { id: "pdfx.output-intent", clause: "CGATS notes 2.16, 3.3, 5.1" };
const OUTPUT_PROFILE: Rule = Rule { id: "pdfx.output-profile", clause: "CGATS notes 3.3, 5.1" };
const PAGE_BOXES: Rule = Rule { id: "pdfx.page-boxes", clause: "CGATS notes 2.10" };
const ANNOTATIONS: Rule = Rule { id: "pdfx.annotations", clause: "CGATS notes 2.28" };
const FONTS: Rule = Rule { id: "pdfx.fonts", clause: "CGATS notes 2.18" };
const COMPRESSION: Rule = Rule { id: "pdfx.compression", clause: "CGATS notes 2.8" };
const EXTERNAL: Rule = Rule { id: "pdfx.external-content", clause: "CGATS notes 1, 3.1.3, 3.2" };
const POSTSCRIPT: Rule = Rule { id: "pdfx.postscript", clause: "CGATS notes 2.26" };
const JAVASCRIPT: Rule = Rule { id: "pdfx.javascript", clause: "CGATS notes 1 (complete exchange)" };
const ACTIONS: Rule = Rule { id: "pdfx.actions", clause: "CGATS notes 1 (complete exchange)" };
const TRANSPARENCY: Rule = Rule { id: "pdfx.transparency", clause: "CGATS notes 2.25" };
const COLOUR: Rule = Rule { id: "pdfx.colour", clause: "CGATS notes 3.3, 5.1" };

/// What [`verify`] has found so far.
#[derive(Default)]
struct Findings(Vec<Finding>);

impl Findings {
    /// Each problem is worded here and named by the document there: the wording goes on the list
    /// in `WORDINGS` for an interface to translate, and the name goes in its `{}` as it stands.
    fn add(&mut self, rule: Rule, pattern: &'static str, value: Option<String>, page: Option<usize>, object: Option<ObjRef>) {
        let message = match &value {
            Some(v) => pattern.replacen("{}", v, 1),
            None => pattern.to_owned(),
        };
        self.0.push(Finding { rule: rule.id, clause: rule.clause, message, pattern, value, page, object: object.map(|r| r.num) });
    }

    /// One finding per kind of problem and page is enough for repeated ones, as PDF/A's are.
    fn done(mut self) -> Vec<Finding> {
        let mut seen = HashSet::new();
        self.0.retain(|f| seen.insert((f.rule, f.message.clone(), f.page)));
        self.0
    }
}

/// Check `doc` against `level`.
pub fn verify(doc: &Document, level: Level) -> Vec<Finding> {
    let mut f = Findings::default();
    let objs = super::objects(doc);
    let pages = pdfcraft_model::pages(doc);
    identification(doc, level, &mut f);
    trailer_id(doc, &mut f);
    trapped(doc, level, &mut f);
    encryption(doc, &mut f);
    let intent = output_intent(doc, level, &mut f);
    page_boxes(doc, &pages, &mut f);
    annotations(doc, &pages, &mut f);
    fonts(doc, &objs, &mut f);
    streams(doc, level, &objs, &mut f);
    actions(doc, &pages, &objs, &mut f);
    if !level.is_x4() {
        transparency(doc, &objs, &mut f);
    }
    colour(doc, level, &objs, &pages, intent, &mut f);
    f.done()
}

/// The PDF/X level a document declares, if it is one of these.
pub fn declared(doc: &Document) -> Option<Level> {
    let version = declared_version(doc)?;
    // PDF/X-1:2001 has two conformance levels, and only PDF/X-1a is checked here.
    if version == Level::X1a2001.label() {
        return Some(Level::X1a2001);
    }
    Level::from_version(&version).filter(|l| *l != Level::X1a2001)
}

/// The PDF/X version a document declares, as it writes it (for the Standards panel):
/// `GTS_PDFXVersion` from the document information, else from the XMP metadata, and for
/// PDF/X-1:2001 its conformance level.
pub(crate) fn declared_version(doc: &Document) -> Option<String> {
    let said = |v: String| (!v.trim().is_empty()).then_some(v);
    let version =
        super::info(doc, "GTS_PDFXVersion").or_else(|| super::metadata(doc).and_then(|x| xmp::value(&x, "pdfxid:GTS_PDFXVersion"))).and_then(said)?;
    if version == Level::X1a2001.version_key()
        && let Some(conformance) = super::info(doc, "GTS_PDFXConformance").and_then(said)
    {
        return Some(conformance);
    }
    Some(version)
}

fn info_object(doc: &Document, key: &[u8]) -> Option<Arc<Object>> {
    let info = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    info.as_dict()?.get(key).map(|v| doc.resolve(v))
}

fn identification(doc: &Document, level: Level, f: &mut Findings) {
    let version = super::info(doc, "GTS_PDFXVersion");
    if !level.is_x4() {
        let conforms = version.as_deref() == Some(level.version_key())
            && (level != Level::X1a2001 || super::info(doc, "GTS_PDFXConformance").as_deref() == Some(Level::X1a2001.label()));
        if !conforms {
            f.add(IDENTIFICATION, "The document information doesn't identify the file as {}", Some(level.label().to_owned()), None, None);
        }
        return;
    }
    // PDF/X-4 identifies itself in its XMP metadata, which it must have.
    let Some(x) = super::metadata(doc) else {
        f.add(IDENTIFICATION, "The document has no XMP metadata", None, None, None);
        return;
    };
    if xmp::value(&x, "pdfxid:GTS_PDFXVersion").as_deref() != Some(level.version_key()) {
        f.add(IDENTIFICATION, "The metadata doesn't identify the file as {}", Some(level.label().to_owned()), None, None);
    }
    if version.is_some_and(|v| v != level.version_key()) {
        f.add(IDENTIFICATION, "The document information and the metadata name different PDF/X versions", None, None, None);
    }
    // The title, the dates and the document's identity (older packets use the `xap` prefixes).
    for (property, older) in [
        ("dc:title", None),
        ("xmp:CreateDate", Some("xap:CreateDate")),
        ("xmp:ModifyDate", Some("xap:ModifyDate")),
        ("xmpMM:DocumentID", Some("xapMM:DocumentID")),
        ("xmpMM:VersionID", Some("xapMM:VersionID")),
        ("xmpMM:RenditionClass", Some("xapMM:RenditionClass")),
    ] {
        let present = |key: &str| xmp::value(&x, key).is_some_and(|v| !v.trim().is_empty());
        if !present(property) && !older.is_some_and(present) {
            f.add(METADATA, "The metadata has no {}", Some(property.to_owned()), None, None);
        }
    }
}

fn trailer_id(doc: &Document, f: &mut Findings) {
    let id = doc.trailer().get(b"ID").map(|o| doc.resolve(o));
    let ok = id.as_deref().and_then(Object::as_array).is_some_and(|a| a.len() == 2 && a.iter().all(|s| doc.resolve(s).as_string().is_some()));
    if !ok {
        f.add(TRAILER_ID, "The file trailer has no document ID", None, None, None);
    }
}

/// How a value the document gives is shown in a message: as PDF writes it, which reads the same
/// in every language.
fn shown(o: &Object) -> String {
    match o {
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Bool(b) => b.to_string(),
        Object::Int(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::String(s) => format!("({})", s.to_text()),
        Object::Null => "null".into(),
        _ => "…".into(),
    }
}

/// `Trapped` is the name `/True` or `/False`: not `/Unknown`, and not a boolean (CGATS notes
/// 2.17). PDF/X-4 may say it in its metadata instead, and where both say it they agree.
fn trapped(doc: &Document, level: Level, f: &mut Findings) {
    let info = info_object(doc, b"Trapped").map(|o| match &*o {
        Object::Name(n) if n == b"True" => Ok(true),
        Object::Name(n) if n == b"False" => Ok(false),
        other => Err(shown(other)),
    });
    let meta = if level.is_x4() {
        super::metadata(doc).and_then(|x| xmp::value(&x, "pdf:Trapped")).map(|t| match t.trim() {
            "True" => Ok(true),
            "False" => Ok(false),
            other => Err(other.to_owned()),
        })
    } else {
        None
    };
    for said in [&info, &meta] {
        if let Some(Err(value)) = said {
            f.add(TRAPPED, "Trapped is {}, but it must be /True or /False", Some(value.clone()), None, None);
        }
    }
    if let (Some(Ok(a)), Some(Ok(b))) = (&info, &meta)
        && a != b
    {
        f.add(TRAPPED, "The Trapped key and the metadata's pdf:Trapped disagree", None, None, None);
    }
    if info.is_none() && meta.is_none() {
        f.add(TRAPPED, "The document doesn't say whether it is trapped (Trapped)", None, None, None);
    }
}

fn encryption(doc: &Document, f: &mut Findings) {
    if doc.trailer().get(b"Encrypt").is_some() {
        f.add(ENCRYPTION, "The document is encrypted", None, None, None);
    }
}

/// The colour space of a printing condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Gray,
    Rgb,
    Cmyk,
}

/// The output intent as far as colour is concerned.
#[derive(Clone, Copy, Debug)]
struct Intent {
    /// The profile's colour space; `None` when no profile says it.
    family: Option<Family>,
    /// Whether it embeds a profile.
    embedded: bool,
}

/// What an ICC profile's header and tag table say.
struct Profile {
    class: [u8; 4],
    space: [u8; 4],
    /// An AToB1 table (device to PCS, colorimetric), or for a gray profile a grayTRC.
    to_pcs: bool,
}

/// Read an ICC profile (ICC.1 header: size at 0, class at 12, colour space at 16, `acsp` at 36;
/// the tag count at 128 and 12-byte tag entries after it). `None` when it isn't one.
fn read_profile(data: &[u8]) -> Option<Profile> {
    let word = |at: usize| -> Option<[u8; 4]> { data.get(at..at.checked_add(4)?)?.try_into().ok() };
    let size = usize::try_from(u32::from_be_bytes(word(0)?)).ok()?;
    if size < 128 || size > data.len() || word(36)? != *b"acsp" {
        return None;
    }
    let (class, space) = (word(12)?, word(16)?);
    let count = usize::try_from(u32::from_be_bytes(word(128)?)).ok()?;
    let table = data.get(132..size)?;
    // A count the profile can't hold is a damaged profile, not a long loop.
    if count > table.len() / 12 {
        return None;
    }
    let (tags, _) = table.as_chunks::<12>();
    let to_pcs = tags.iter().take(count).any(|tag| {
        let sig = tag.get(..4);
        sig == Some(b"A2B1".as_slice()) || (space == *b"GRAY" && sig == Some(b"kTRC".as_slice()))
    });
    Some(Profile { class, space, to_pcs })
}

/// Whether an output intent points at a characterization in the ICC registry.
fn registered(doc: &Document, oi: &Dict) -> bool {
    oi.get(b"RegistryName")
        .and_then(|r| doc.resolve(r).as_string().map(|s| s.to_text()))
        .is_some_and(|r| r.to_ascii_lowercase().contains("color.org"))
}

fn output_intent(doc: &Document, level: Level, f: &mut Findings) -> Option<Intent> {
    let cat = super::catalog(doc);
    let list = cat.get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    let pdfx: Vec<(Option<ObjRef>, Dict)> = list
        .iter()
        .filter_map(|oi| Some((oi.as_ref(), doc.resolve(oi).as_dict().cloned()?)))
        .filter(|(_, d)| d.get(b"S").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) == Some(b"GTS_PDFX".as_slice()))
        .collect();
    let Some((at, oi)) = pdfx.first() else {
        f.add(OUTPUT_INTENT, "There is no PDF/X output intent (GTS_PDFX)", None, None, None);
        return None;
    };
    let at = *at;
    if pdfx.len() > 1 {
        f.add(OUTPUT_INTENT, "There is more than one PDF/X output intent", None, None, at);
    }
    if oi.get(b"OutputConditionIdentifier").and_then(|i| doc.resolve(i).as_string().cloned()).is_none() {
        f.add(OUTPUT_INTENT, "The PDF/X output intent has no OutputConditionIdentifier", None, None, at);
    }
    let Some(profile_at) = oi.get(b"DestOutputProfile") else {
        // PDF/X-1a and -3 may name a characterization in the ICC registry instead (CGATS notes
        // 3.3, 5.1); PDF/X-4 embeds its profile.
        if level.is_x4() {
            f.add(OUTPUT_INTENT, "The PDF/X output intent has no embedded profile (DestOutputProfile)", None, None, at);
        } else if !registered(doc, oi) {
            f.add(
                OUTPUT_INTENT,
                "The PDF/X output intent has neither an embedded profile nor a characterization in the ICC registry",
                None,
                None,
                at,
            );
        }
        return Some(Intent { family: level.cmyk_only().then_some(Family::Cmyk), embedded: false });
    };
    let at = profile_at.as_ref().or(at);
    let stream = match &*doc.resolve(profile_at) {
        Object::Stream(s) => Some(s.clone()),
        _ => None,
    };
    let Some(profile) = stream.as_ref().and_then(|s| read_profile(&s.decoded().ok()?)) else {
        f.add(OUTPUT_PROFILE, "The output intent's profile isn't a valid ICC profile", None, None, at);
        return Some(Intent { family: None, embedded: true });
    };
    if profile.class != *b"prtr" {
        f.add(OUTPUT_PROFILE, "The output intent's profile isn't an output (printer) profile", None, None, at);
    }
    let (family, components) = match &profile.space {
        b"GRAY" => (Some(Family::Gray), 1),
        b"RGB " => (Some(Family::Rgb), 3),
        b"CMYK" => (Some(Family::Cmyk), 4),
        _ => (None, 0),
    };
    let space = String::from_utf8_lossy(&profile.space).trim().to_owned();
    if level.cmyk_only() && family != Some(Family::Cmyk) {
        f.add(OUTPUT_PROFILE, "The output intent's profile describes {} colour, but PDF/X-1a needs a CMYK printing condition", Some(space), None, at);
    } else if family.is_none() {
        f.add(OUTPUT_PROFILE, "The output intent's profile describes {} colour; it must be gray, RGB or CMYK", Some(space), None, at);
    }
    if let Some(n) = stream.as_ref().and_then(|s| s.dict.int(b"N"))
        && family.is_some()
        && n != components
    {
        f.add(OUTPUT_PROFILE, "The output intent's profile has /N {}, which doesn't match its colour space", Some(n.to_string()), None, at);
    }
    if !profile.to_pcs {
        f.add(OUTPUT_PROFILE, "The output intent's profile has no device-to-PCS transform (AToB1 or grayTRC)", None, None, at);
    }
    Some(Intent { family, embedded: true })
}

/// A rectangle, normalized so its corners are lower-left then upper-right.
fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        &[a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
        _ => None,
    }
}

/// How far a box may stray past another before it counts: writers round to a few decimals.
const EPS: f64 = 0.01;

fn inside(inner: [f64; 4], outer: [f64; 4]) -> bool {
    inner[0] >= outer[0] - EPS && inner[1] >= outer[1] - EPS && inner[2] <= outer[2] + EPS && inner[3] <= outer[3] + EPS
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] - EPS && a[2] > b[0] + EPS && a[1] < b[3] - EPS && a[3] > b[1] + EPS
}

/// A TrimBox or an ArtBox but not both, nested inside the BleedBox, the CropBox and the
/// MediaBox (CGATS notes 2.10), and the BleedBox inside the MediaBox (ISO 32000 14.11.2).
fn page_boxes(doc: &Document, pages: &[pdfcraft_model::Page], f: &mut Findings) {
    for (i, p) in pages.iter().enumerate() {
        let (page, at) = (Some(i), Some(p.obj));
        let boxed = |key: &[u8]| rect(doc, p.dict.get(key));
        let Some(media) = boxed(b"MediaBox") else {
            f.add(PAGE_BOXES, "The page has no valid MediaBox", None, page, at);
            continue;
        };
        let (crop, bleed, trim, art) = (boxed(b"CropBox"), boxed(b"BleedBox"), boxed(b"TrimBox"), boxed(b"ArtBox"));
        match (trim, art) {
            (None, None) => f.add(PAGE_BOXES, "The page has neither a TrimBox nor an ArtBox", None, page, at),
            (Some(_), Some(_)) => f.add(PAGE_BOXES, "The page has both a TrimBox and an ArtBox", None, page, at),
            _ => {}
        }
        if let Some(b) = bleed
            && !inside(b, media)
        {
            f.add(PAGE_BOXES, "The {} extends beyond the MediaBox", Some("BleedBox".into()), page, at);
        }
        for (name, finished) in [("TrimBox", trim), ("ArtBox", art)] {
            let Some(r) = finished else { continue };
            if let Some(b) = bleed
                && !inside(r, b)
            {
                f.add(PAGE_BOXES, "The {} extends beyond the BleedBox", Some(name.into()), page, at);
            }
            if let Some(c) = crop
                && !inside(r, c)
            {
                f.add(PAGE_BOXES, "The {} extends beyond the CropBox", Some(name.into()), page, at);
            }
            if !inside(r, media) {
                f.add(PAGE_BOXES, "The {} extends beyond the MediaBox", Some(name.into()), page, at);
            }
        }
    }
}

/// Annotations lie entirely outside the BleedBox; printer marks may lie inside it but outside
/// the TrimBox or ArtBox (CGATS notes 2.28). The boxes default as ISO 32000 14.11.2 says: the
/// BleedBox, TrimBox and ArtBox to the CropBox, and that to the MediaBox.
fn annotations(doc: &Document, pages: &[pdfcraft_model::Page], f: &mut Findings) {
    for (i, p) in pages.iter().enumerate() {
        let boxed = |key: &[u8]| rect(doc, p.dict.get(key));
        let Some(media) = boxed(b"MediaBox") else { continue };
        let crop = boxed(b"CropBox").unwrap_or(media);
        let bleed = boxed(b"BleedBox").unwrap_or(crop);
        let finished = boxed(b"TrimBox").or_else(|| boxed(b"ArtBox")).unwrap_or(crop);
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in &annots {
            let d = doc.resolve(a);
            let Some(d) = d.as_dict() else { continue };
            // A pop-up belongs to the annotation that opens it, and trap networks are print data.
            let Some(sub) = d.name(b"Subtype").filter(|s| !matches!(*s, b"Popup" | b"TrapNet")) else { continue };
            // A rectangle with no area draws nothing.
            let Some(r) = rect(doc, d.get(b"Rect")).filter(|r| r[2] - r[0] > EPS && r[3] - r[1] > EPS) else { continue };
            if sub == b"PrinterMark" {
                if overlaps(r, finished) {
                    f.add(ANNOTATIONS, "A PrinterMark annotation overlaps the TrimBox or ArtBox", None, Some(i), a.as_ref());
                }
            } else if overlaps(r, bleed) {
                f.add(ANNOTATIONS, "A {} annotation lies within the BleedBox", Some(String::from_utf8_lossy(sub).into_owned()), Some(i), a.as_ref());
            }
        }
    }
}

/// Every font embedded (CGATS notes 2.18), as PDF/A's check decides it.
fn fonts(doc: &Document, objs: &[(ObjRef, Arc<Object>)], f: &mut Findings) {
    for (r, o) in objs {
        let Some(d) = super::dict_of(o) else { continue };
        if d.name(b"Type") == Some(b"Font") || d.get(b"BaseFont").is_some() && d.name(b"Subtype").is_some() {
            super::check_font(doc, d, &mut |_clause, pattern, value, _page, _fixable| f.add(FONTS, pattern, value, None, Some(*r)));
        }
    }
}

/// Compression, external data, OPI and PostScript: what a stream's dictionary says.
fn streams(doc: &Document, level: Level, objs: &[(ObjRef, Arc<Object>)], f: &mut Findings) {
    for (r, o) in objs {
        let Object::Stream(s) = &**o else { continue };
        let at = Some(*r);
        let filter = s.dict.get(b"Filter").map(|x| doc.resolve(x));
        let filters: Vec<&[u8]> = match filter.as_deref() {
            Some(Object::Name(n)) => vec![n.as_slice()],
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
            _ => Vec::new(),
        };
        if filters.contains(&b"LZWDecode".as_slice()) {
            f.add(COMPRESSION, "A stream uses LZW compression", None, None, at);
        }
        if !level.is_x4() && filters.contains(&b"JBIG2Decode".as_slice()) {
            f.add(COMPRESSION, "A stream uses JBIG2 compression", None, None, at);
        }
        if s.dict.get(b"F").is_some() || s.dict.get(b"FFilter").is_some() {
            f.add(EXTERNAL, "A stream refers to an external file", None, None, at);
        }
        let sub = s.dict.name(b"Subtype");
        if sub == Some(b"Form") && s.dict.get(b"Ref").is_some() {
            f.add(EXTERNAL, "A form XObject refers to a page of another file (reference XObject)", None, None, at);
        }
        if matches!(sub, Some(b"Image" | b"Form")) && s.dict.get(b"OPI").is_some() {
            f.add(EXTERNAL, "An image or form has OPI information", None, None, at);
        }
        if sub == Some(b"PS") || sub == Some(b"Form") && s.dict.name(b"Subtype2") == Some(b"PS") {
            f.add(POSTSCRIPT, "PostScript XObjects aren't allowed", None, None, at);
        }
    }
}

/// Whether `o` is (or refers to) a JavaScript action.
fn is_javascript(doc: &Document, o: &Object) -> bool {
    doc.resolve(o).as_dict().is_some_and(|a| a.name(b"S") == Some(b"JavaScript") && super::is_action(a))
}

/// No JavaScript, and nothing that runs by itself (additional actions): what a file does when it
/// is opened or printed must not change what it prints.
fn actions(doc: &Document, pages: &[pdfcraft_model::Page], objs: &[(ObjRef, Arc<Object>)], f: &mut Findings) {
    let cat = super::catalog(doc);
    if cat.get(b"AA").is_some() {
        f.add(ACTIONS, "The document has additional actions (/AA)", None, None, doc.root());
    }
    if let Some(names) = cat.get(b"Names").map(|n| doc.resolve(n))
        && names.as_dict().is_some_and(|n| n.get(b"JavaScript").is_some())
    {
        f.add(JAVASCRIPT, "The document has document-level JavaScript", None, None, doc.root());
    }
    if cat.get(b"OpenAction").is_some_and(|a| is_javascript(doc, a)) {
        f.add(JAVASCRIPT, "A {} action isn't allowed", Some("JavaScript".into()), None, doc.root());
    }
    let mut annot_page = HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        if p.dict.get(b"AA").is_some() {
            f.add(ACTIONS, "Page {} has additional actions", Some((i + 1).to_string()), Some(i), Some(p.obj));
        }
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        annot_page.extend(annots.iter().filter_map(Object::as_ref).map(|r| (r, i)));
    }
    for (r, o) in objs {
        let Some(d) = super::dict_of(o) else { continue };
        let page = annot_page.get(r).copied();
        if d.name(b"S") == Some(b"JavaScript") && super::is_action(d) {
            f.add(JAVASCRIPT, "A {} action isn't allowed", Some("JavaScript".into()), None, Some(*r));
        }
        let is_annot = d.name(b"Type") == Some(b"Annot") || page.is_some();
        // An action written into the annotation or field rather than as an object of its own.
        if (is_annot || d.get(b"FT").is_some()) && d.get(b"A").is_some_and(|a| is_javascript(doc, a)) {
            f.add(JAVASCRIPT, "A {} action isn't allowed", Some("JavaScript".into()), page, Some(*r));
        }
        if d.get(b"AA").is_some() {
            if is_annot {
                let sub = d.name(b"Subtype").map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
                f.add(ACTIONS, "A {} annotation has additional actions", Some(sub), page, Some(*r));
            } else if d.get(b"FT").is_some() {
                f.add(ACTIONS, "A form field has additional actions", None, None, Some(*r));
            }
        }
    }
}

/// Check one graphics state for transparency.
fn graphics_state(g: &Dict, at: ObjRef, f: &mut Findings) {
    if g.get(b"SMask").is_some_and(|m| m.as_name() != Some(b"None")) {
        f.add(TRANSPARENCY, "A graphics state uses a soft mask", None, None, Some(at));
    }
    if [b"CA".as_slice(), b"ca".as_slice()].iter().any(|k| g.get(k).and_then(Object::as_f64).is_some_and(|a| a < 1.0 - 1e-6)) {
        f.add(TRANSPARENCY, "A graphics state sets an opacity below 1", None, None, Some(at));
    }
    let modes: Vec<&[u8]> = match g.get(b"BM") {
        Some(Object::Name(n)) => vec![n.as_slice()],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
        _ => Vec::new(),
    };
    if let Some(mode) = modes.into_iter().find(|m| !matches!(*m, b"Normal" | b"Compatible")) {
        f.add(TRANSPARENCY, "A graphics state uses the {} blend mode", Some(String::from_utf8_lossy(mode).into_owned()), None, Some(at));
    }
}

/// The resource dictionaries `d` holds: itself, when it is one, and its inline `/Resources`.
/// Resources held by reference are objects of their own, reached when those are.
fn resource_holders(d: &Dict) -> [Option<&Dict>; 2] {
    let inline = match d.get(b"Resources") {
        Some(Object::Dict(r)) => Some(r),
        _ => None,
    };
    [Some(d), inline]
}

/// Partial transparency (CGATS notes 2.25): soft masks, opacity, blend modes other than Normal,
/// and images with soft masks.
fn transparency(doc: &Document, objs: &[(ObjRef, Arc<Object>)], f: &mut Findings) {
    for (r, o) in objs {
        let Some(d) = super::dict_of(o) else { continue };
        for holder in resource_holders(d).into_iter().flatten() {
            let Some(states) = holder.get(b"ExtGState").map(|s| doc.resolve(s)) else { continue };
            for (_, g) in states.as_dict().into_iter().flat_map(Dict::iter) {
                if let Some(state) = doc.resolve(g).as_dict() {
                    graphics_state(state, g.as_ref().unwrap_or(*r), f);
                }
            }
        }
        if let Object::Stream(s) = &**o
            && s.dict.name(b"Subtype") == Some(b"Image")
            && (s.dict.get(b"SMask").is_some_and(|m| !matches!(&*doc.resolve(m), Object::Null)) || s.dict.int(b"SMaskInData").is_some_and(|n| n > 0))
        {
            f.add(TRANSPARENCY, "An image has a soft mask", None, None, Some(*r));
        }
    }
}

/// How deep [`families`] follows a colour space to the ones under it: an indexed space over a
/// Separation under a pattern is three steps, and a deeper or cyclic chain is malformed.
const MAX_SPACE_DEPTH: u8 = 4;

const DEVICE: [&str; 3] = ["DeviceGray", "DeviceRGB", "DeviceCMYK"];
const INDEPENDENT: [&str; 4] = ["ICCBased", "CalGray", "CalRGB", "Lab"];

fn family(name: &[u8]) -> Option<&'static str> {
    Some(match name {
        b"DeviceGray" => "DeviceGray",
        b"DeviceRGB" => "DeviceRGB",
        b"DeviceCMYK" => "DeviceCMYK",
        b"ICCBased" => "ICCBased",
        b"CalGray" => "CalGray",
        b"CalRGB" => "CalRGB",
        b"Lab" => "Lab",
        b"Indexed" => "Indexed",
        b"Pattern" => "Pattern",
        b"Separation" => "Separation",
        b"DeviceN" => "DeviceN",
        _ => return None,
    })
}

/// The colour space families `cs` uses: its own, and those of the base of an Indexed or Pattern
/// space and the alternate of a Separation or DeviceN space, which draw it on any device without
/// its colorants (CGATS notes 5.1 applies the colour rules to them too).
fn families(doc: &Document, cs: &Object, depth: u8, out: &mut Vec<&'static str>) {
    if depth > MAX_SPACE_DEPTH {
        return;
    }
    let cs = doc.resolve(cs);
    let (name, under) = match &*cs {
        Object::Name(n) => (n.as_slice(), None),
        Object::Array(a) => {
            let Some(first) = a.first().and_then(Object::as_name) else { return };
            let under = match first {
                b"Indexed" | b"Pattern" => a.get(1),
                b"Separation" | b"DeviceN" => a.get(2),
                _ => None,
            };
            (first, under)
        }
        _ => return,
    };
    if let Some(fam) = family(name) {
        out.push(fam);
    }
    if let Some(under) = under {
        families(doc, under, depth.saturating_add(1), out);
    }
}

/// A colour space family an object uses, and where.
struct SpaceUse {
    family: &'static str,
    at: ObjRef,
    page: Option<usize>,
}

/// Which device spaces a `Default` space maps (ISO 32000 8.6.5.6).
#[derive(Default)]
struct Defaults {
    gray: bool,
    rgb: bool,
    cmyk: bool,
}

/// The colour space of the shading written into the shading pattern `p`. A shading of its own is
/// an object, checked where it is however many patterns and lists name it.
fn written_shading(p: &Dict) -> Option<&Object> {
    match p.get(b"Shading") {
        Some(Object::Dict(s)) if s.get(b"ShadingType").is_some() => s.get(b"ColorSpace"),
        _ => None,
    }
}

/// The colour spaces the document defines: images' and shadings' own, every space a resource
/// dictionary lists, and the spaces of the shadings its `/Shading` and `/Pattern` lists hold
/// (listed counts as used, whether or not content draws it with `sh` or selects it with `scn`).
fn defined_spaces(doc: &Document, objs: &[(ObjRef, Arc<Object>)], pages: &[pdfcraft_model::Page]) -> (Vec<SpaceUse>, Defaults) {
    let page_of: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let mut uses = Vec::new();
    let mut defaults = Defaults::default();
    for (r, o) in objs {
        let Some(d) = super::dict_of(o) else { continue };
        let mut found = Vec::new();
        // An image's or a shading's own space (a resource dictionary's /ColorSpace is a list).
        if let Some(cs) = d.get(b"ColorSpace")
            && doc.resolve(cs).as_dict().is_none()
        {
            families(doc, cs, 0, &mut found);
        }
        // A pattern's shading, written into it.
        if let Some(cs) = written_shading(d) {
            families(doc, cs, 0, &mut found);
        }
        for holder in resource_holders(d).into_iter().flatten() {
            if let Some(list) = holder.get(b"ColorSpace").map(|c| doc.resolve(c))
                && let Some(list) = list.as_dict()
            {
                for (name, cs) in list.iter() {
                    match name.as_slice() {
                        b"DefaultGray" => defaults.gray = true,
                        b"DefaultRGB" => defaults.rgb = true,
                        b"DefaultCMYK" => defaults.cmyk = true,
                        _ => {}
                    }
                    families(doc, cs, 0, &mut found);
                }
            }
            // Shadings written into a resource dictionary's /Shading list.
            if let Some(list) = holder.get(b"Shading").map(|c| doc.resolve(c))
                && let Some(list) = list.as_dict().filter(|l| l.get(b"ShadingType").is_none())
            {
                for (_, shading) in list.iter() {
                    if let Object::Dict(shading) = shading
                        && let Some(cs) = shading.get(b"ColorSpace")
                    {
                        families(doc, cs, 0, &mut found);
                    }
                }
            }
            // Shading patterns written into a resource dictionary's /Pattern list, and the
            // shadings written into them. A pattern of its own is an object, checked above.
            if let Some(list) = holder.get(b"Pattern").map(|p| doc.resolve(p))
                && let Some(list) = list.as_dict()
            {
                for (_, pattern) in list.iter() {
                    if let Object::Dict(pattern) = pattern
                        && let Some(cs) = written_shading(pattern)
                    {
                        families(doc, cs, 0, &mut found);
                    }
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        uses.extend(found.into_iter().map(|family| SpaceUse { family, at: *r, page: page_of.get(r).copied() }));
    }
    (uses, defaults)
}

/// PDF/X-1a: CMYK, gray and spot colour only (CGATS notes 3.1.1, 3.3). PDF/X-3 and -4: device
/// colour in the output intent's colour space, or DeviceGray under a CMYK one, unless a `Default`
/// space maps it; device-independent colour needs the output intent to embed its profile (5.1).
fn colour(doc: &Document, level: Level, objs: &[(ObjRef, Arc<Object>)], pages: &[pdfcraft_model::Page], intent: Option<Intent>, f: &mut Findings) {
    let (uses, defaults) = defined_spaces(doc, objs, pages);
    let content = super::uses_device_colour(doc, objs);
    let defined = |family: &str| uses.iter().any(|u| u.family == family);
    // Device colour that no definition explains was set by content operators.
    let set_by_content = [("DeviceGray", content.gray), ("DeviceRGB", content.rgb), ("DeviceCMYK", content.cmyk)];
    if level.cmyk_only() {
        for u in uses.iter().filter(|u| u.family == "DeviceRGB" || INDEPENDENT.contains(&u.family)) {
            f.add(
                COLOUR,
                "The {} colour space isn't allowed: PDF/X-1a allows only CMYK, gray and spot colour",
                Some(u.family.into()),
                u.page,
                Some(u.at),
            );
        }
        if content.rgb && !defined("DeviceRGB") {
            f.add(COLOUR, "Content uses DeviceRGB colour: PDF/X-1a allows only CMYK, gray and spot colour", None, None, None);
        }
        return;
    }
    let Some(intent) = intent else { return };
    if !level.is_x4() && !intent.embedded && uses.iter().any(|u| INDEPENDENT.contains(&u.family)) {
        f.add(OUTPUT_INTENT, "Device-independent colour is used, so the output intent needs an embedded profile", None, None, None);
    }
    // Without a profile there is no colour space to match device colour against.
    let Some(target) = intent.family else { return };
    let fits = |family: &str| match family {
        "DeviceGray" => matches!(target, Family::Gray | Family::Cmyk) || defaults.gray,
        "DeviceRGB" => target == Family::Rgb || defaults.rgb,
        "DeviceCMYK" => target == Family::Cmyk || defaults.cmyk,
        _ => true,
    };
    for u in uses.iter().filter(|u| DEVICE.contains(&u.family) && !fits(u.family)) {
        f.add(COLOUR, "{} colour doesn't match the output intent's colour space", Some(u.family.into()), u.page, Some(u.at));
    }
    for (family, used) in set_by_content {
        if used && !defined(family) && !fits(family) {
            f.add(COLOUR, "Content uses {} colour, which doesn't match the output intent's colour space", Some(family.into()), None, None);
        }
    }
}

#[cfg(test)]
mod tests;
