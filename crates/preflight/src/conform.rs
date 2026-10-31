//! Save as PDF/X and Save as PDF/A: convert a document so it conforms to a level, or refuse and
//! say why.
//!
//! A conversion works on a copy, which replaces the document only when the level's verifier
//! ([`crate::pdfx::verify`], [`crate::verify`]) then finds nothing. Otherwise the document is left
//! as it was and the [`Refusal`] lists what stands in the way: what the conversion can't do, then
//! what the verifier still finds. A document is never left claiming a level it fails.
//!
//! PDF/X ([`pdfx`]):
//! - the printing condition becomes the one `GTS_PDFX` output intent: a characterization in the
//!   ICC registry (PDF/X-1a and -3 only; FOGRA39 unless another is chosen), or the user's ICC
//!   profile, embedded (PDF/X-4 needs one, and so does converting colour);
//! - identification: `GTS_PDFXVersion` (and `GTS_PDFXConformance` for PDF/X-1a:2001) in the
//!   document information, `Trapped` (`/False` unless the document says it is trapped), a
//!   document ID in the trailer, and XMP metadata with the title, the dates and the document's
//!   identity, and for PDF/X-4 `pdfxid:GTS_PDFXVersion`;
//! - page boxes: a TrimBox (the CropBox when the page has neither a TrimBox nor an ArtBox; with
//!   both, the ArtBox goes), a BleedBox (the CropBox when there is none), nested inside the CropBox
//!   and the MediaBox;
//! - JavaScript and additional actions removed; LZW streams re-encoded with Flate; OPI comments and
//!   reference-XObject links dropped (the proxies they stand for stay); with
//!   [`Options::drop_unprinted`], annotations in the bleed that print nothing (links, hidden or
//!   non-printing ones) removed;
//! - colour: PDF/X-1a converts RGB and ICC-based colour to CMYK through the output profile (see
//!   `recolour.rs` for what it can and can't); PDF/X-3 and -4 keep device-independent colour, and
//!   device colour the printing condition's colour space doesn't hold gets a `Default` space: sRGB
//!   for RGB, a gray profile for gray under an RGB condition;
//! - the PDF version: PDF/X-1a:2001 and PDF/X-3:2002 are PDF 1.3, the 2003 levels PDF 1.4 and
//!   PDF/X-4 PDF 1.6, so the header says no more, and features of later versions (JPEG 2000,
//!   layers, OpenType font programs) refuse the conversion; for PDF/X-4, each optional-content
//!   configuration gets a `Name` and loses `AS` (from public summaries of ISO 15930-7).
//!
//! It can't embed fonts that aren't embedded, flatten transparency, or recompress JBIG2.
//!
//! PDF/A ([`pdfa`]): [`crate::convert`]'s fixes; with a chosen ICC profile, that profile as the
//! output intent (how a document with CMYK device colour gets one), with sRGB as `DefaultRGB`
//! for RGB under a CMYK condition; for level a, `MarkInfo` on a document with a structure tree;
//! a header no newer than PDF 1.7. Levels u and a can't add Unicode maps or tags: a document that
//! lacks them is refused.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::output::{CHARACTERIZATIONS, Colorants, OutputCondition};
use crate::{Level, Words, pdfx, walk, xmp};

/// How a conversion goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// The printing condition: PDF/X's output intent, and for PDF/A an output intent to set (a
    /// document with CMYK device colour needs one). `None`: the document's own PDF/X output
    /// intent ([`printing_condition`]) when it has one, else PDF/X-1a and -3 name FOGRA39 and
    /// PDF/X-4 refuses; PDF/A keeps the document's intent or adds sRGB.
    pub condition: Option<OutputCondition>,
    /// PDF/X: remove the annotations in the bleed that print nothing (links, hidden or
    /// non-printing annotations); others there still refuse the conversion.
    pub drop_unprinted: bool,
    /// The title for a document that has none (PDF/X-4 metadata needs one): its file name, say.
    pub title: Option<String>,
    /// Now, as a PDF date (`D:20261016120000Z`): the modification date the document information
    /// and the metadata both get, and the creation date when the document has none.
    pub now: Option<String>,
}

/// Something that stands in the way of a conversion.
///
/// Like the verifiers' findings, [`message`](Blocker::message) is the English sentence, and an
/// interface in another language says it from [`pattern`](Blocker::pattern) (in [`WORDINGS`], or
/// the verifiers' wordings) with [`value`](Blocker::value) put in: [`Blocker::said`].
#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    /// Where the rule is, when a verifier found it: the ISO 19005-2 clause, or the PDF/X rule id
    /// and the section of the CGATS notes. `None` for what the conversion itself can't do.
    pub rule: Option<String>,
    pub message: String,
    pub pattern: &'static str,
    pub value: Option<String>,
    /// 0-based page, when it is on a page.
    pub page: Option<usize>,
}

impl Blocker {
    pub(crate) fn new(pattern: &'static str, value: Option<String>, page: Option<usize>) -> Blocker {
        let message = match &value {
            Some(v) => pattern.replacen("{}", v, 1),
            None => pattern.to_owned(),
        };
        Blocker { rule: None, message, pattern, value, page }
    }

    /// The sentence in the language `words` translates into.
    pub fn said(&self, words: Words) -> String {
        let said = words(self.pattern);
        match &self.value {
            Some(v) => said.replacen("{}", v, 1),
            None => said,
        }
    }
}

impl From<&pdfx::Finding> for Blocker {
    fn from(f: &pdfx::Finding) -> Blocker {
        Blocker {
            rule: Some(format!("{} ({})", f.rule, f.clause)),
            message: f.message.clone(),
            pattern: f.pattern,
            value: f.value.clone(),
            page: f.page,
        }
    }
}

impl From<&crate::Issue> for Blocker {
    fn from(i: &crate::Issue) -> Blocker {
        Blocker {
            rule: Some(format!("ISO 19005-2 {}", i.clause)),
            message: i.message.clone(),
            pattern: i.pattern,
            value: i.value.clone(),
            page: i.page,
        }
    }
}

/// Why a document wasn't converted. The document is unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    /// The level asked for, as its label (`PDF/X-4`).
    pub level: &'static str,
    pub blockers: Vec<Blocker>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reasons: Vec<String> = self
            .blockers
            .iter()
            .map(|b| match b.page {
                Some(p) => format!("{} (page {})", b.message, p.saturating_add(1)),
                None => b.message.clone(),
            })
            .collect();
        write!(f, "can't save as {}: {}", self.level, reasons.join("; "))
    }
}

impl std::error::Error for Refusal {}

/// Everything a conversion can say stands in its way, in English, with `{}` where the document's
/// own name goes. What the verifiers still find is worded in their lists ([`crate::WORDINGS`],
/// [`pdfx::WORDINGS`]).
pub const WORDINGS: &[&str] = &[
    // The document and the printing condition.
    "The document is encrypted: open it with its permissions password to remove the security",
    "The document can't be converted: {}",
    "PDF/X-4 embeds the printing condition's ICC profile: choose a profile file",
    "PDF/A embeds the output intent's ICC profile: choose a profile file",
    "The ICC profile {} isn't an output (printer) profile",
    "The ICC profile {} isn't an output or display profile",
    "The ICC profile {} isn't a CMYK profile, which PDF/X-1a needs",
    "The ICC profile {} isn't a gray, RGB or CMYK profile",
    "Converting colour to CMYK needs the printing condition's ICC profile: choose a profile file",
    "{} needs the printing condition's ICC profile embedded for RGB or device-independent colour: choose a profile file",
    "CMYK colour needs a CMYK printing condition",
    "The document uses CMYK colour: choose the ICC profile of a CMYK printing condition for its output intent",
    // What a PDF version allows.
    "JPEG 2000 pictures need PDF 1.5, newer than {} allows",
    "Layers (optional content) need PDF 1.5, newer than {} allows",
    "OpenType font programs need PDF 1.6, newer than {} allows",
    // Pages.
    "Page {}'s TrimBox, BleedBox and CropBox don't overlap",
    // Converting colour to CMYK.
    "The output profile can't convert colour to CMYK ({})",
    "A colour space's ICC profile can't be used to convert its colour",
    "A colour space is nested too deeply to convert",
    "The spot colour {} has an alternate that can't be converted to CMYK yet",
    "A pattern over RGB or device-independent colour can't be converted to CMYK yet",
    "{} colour can't be converted to CMYK yet",
    "A content stream can't be read, so its colour can't be converted",
    "A colour operator's operands don't match its colour space",
    "A content stream is drawn with different resources in different places, so its colour can't be converted",
    "A shading in RGB or device-independent colour can't be converted to CMYK yet",
    "A picture with other than 8 bits per component can't be converted to CMYK yet",
    "A picture with a decode array or a colour-key mask can't be converted to CMYK yet",
    "A picture's data can't be read, so it can't be converted to CMYK",
    "A JPEG picture in CMYK can't be converted yet",
    "An indexed colour space's table is too short to convert",
];

/// The newest PDF version a PDF/X level allows (its base: CGATS notes 1).
pub fn pdf_version(level: pdfx::Level) -> &'static str {
    match level {
        pdfx::Level::X1a2001 | pdfx::Level::X3_2002 => "1.3",
        pdfx::Level::X1a2003 | pdfx::Level::X3_2003 => "1.4",
        pdfx::Level::X4 => "1.6",
    }
}

/// The newest PDF version the standards a document declares allow, if they cap it: a save must
/// not write a newer header, or object streams before PDF 1.5.
pub fn max_version(doc: &Document) -> Option<&'static str> {
    let x = pdfx::declared(doc).map(pdf_version);
    let a = crate::declared(doc).pdfa.map(|(part, _)| if part == 1 { "1.4" } else { "1.7" });
    match (x, a) {
        (Some(x), Some(a)) => Some(x.min(a)),
        (x, a) => x.or(a),
    }
}

/// The printing condition `doc`'s PDF/X output intent names, if it has one PdfCraft can use: its
/// embedded profile (named as the intent names it), or an ICC registry characterization.
pub fn printing_condition(doc: &Document) -> Option<OutputCondition> {
    let cat = crate::catalog(doc);
    let list = cat.get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    let oi = list.iter().find_map(|oi| doc.resolve(oi).as_dict().filter(|d| d.name(b"S") == Some(b"GTS_PDFX")).cloned())?;
    let identifier = oi.get(b"OutputConditionIdentifier").and_then(|i| doc.resolve(i).as_string().map(|s| s.to_text()));
    match oi.get(b"DestOutputProfile").map(|p| doc.resolve(p)).as_deref() {
        Some(Object::Stream(s)) => {
            let profile = crate::output::IccProfile::read(s.decoded_strict().ok()?).ok()?;
            Some(OutputCondition::profile(profile, identifier))
        }
        _ => OutputCondition::registered(&identifier?),
    }
}

/// Convert `doc` to PDF/X `level`. On success returns what was changed; on refusal `doc` is
/// unchanged.
pub fn pdfx(doc: &mut Document, level: pdfx::Level, opts: &Options) -> Result<Vec<String>, Refusal> {
    let mut w = Work { doc: doc.clone(), fixed: Vec::new(), blockers: Vec::new(), label: level.label() };
    w.security();
    let condition = match (opts.condition.clone().or_else(|| printing_condition(doc)), level) {
        (Some(c), _) => Some(c),
        (None, pdfx::Level::X4) => None,
        (None, _) => CHARACTERIZATIONS.first().copied().map(OutputCondition::Registered),
    };
    // PDF/X-4 embeds its profile: neither nothing nor a registry name will do.
    if level.is_x4() && condition.as_ref().is_none_or(|c| c.icc().is_none()) {
        w.block("PDF/X-4 embeds the printing condition's ICC profile: choose a profile file", None, None);
    }
    if let Some(icc) = condition.as_ref().and_then(OutputCondition::icc) {
        let label = condition.as_ref().map(OutputCondition::label);
        if !icc.is_output() {
            w.block("The ICC profile {} isn't an output (printer) profile", label, None);
        } else if level.cmyk_only() && icc.colorants != Some(Colorants::Cmyk) {
            w.block("The ICC profile {} isn't a CMYK profile, which PDF/X-1a needs", label, None);
        } else if icc.colorants.is_none() {
            w.block("The ICC profile {} isn't a gray, RGB or CMYK profile", label, None);
        }
    }
    let version = pdf_version(level);
    w.version_features(version);
    w.inline_fonts();
    w.scripts();
    w.streams();
    w.page_boxes();
    if opts.drop_unprinted {
        w.unprinted_annotations();
    }
    let mut profile = None;
    if let Some(c) = &condition {
        w.reuse_profile(c, &mut profile);
        if level.cmyk_only() {
            if let Some(icc) = c.icc() {
                w.convert_to_cmyk(&icc.bytes, &c.label());
            }
        } else if let Some(family) = c.colorants() {
            // PDF/X-4 always embeds its profile; PDF/X-3 may name a registered one instead, and
            // then has no profile to describe device-independent colour with.
            w.default_spaces(family, level.is_x4() || c.icc().is_some());
        }
        w.set_intent("GTS_PDFX", c, &mut profile);
    }
    let trapped = w.pdfx_information(level, opts);
    w.trailer_id(opts);
    w.pdfx_metadata(level, trapped, opts);
    w.version(version);
    if level.is_x4() {
        w.optional_content();
    }
    let findings = pdfx::verify(&w.doc, level);
    // A registered condition has no profile to convert colour with: say so before the colour
    // the verifier finds.
    if level.cmyk_only() && condition.as_ref().is_some_and(|c| c.icc().is_none()) && findings.iter().any(|f| f.rule == "pdfx.colour") {
        w.block("Converting colour to CMYK needs the printing condition's ICC profile: choose a profile file", None, None);
    }
    w.blockers.extend(findings.iter().map(Blocker::from));
    w.finish(doc)
}

/// Convert `doc` to PDF/A `level`. On success returns what was changed; on refusal `doc` is
/// unchanged.
pub fn pdfa(doc: &mut Document, level: Level, opts: &Options) -> Result<Vec<String>, Refusal> {
    let mut w = Work { doc: doc.clone(), fixed: Vec::new(), blockers: Vec::new(), label: level.label() };
    w.security();
    let used = crate::uses_device_colour(&w.doc, &crate::objects(&w.doc));
    match &opts.condition {
        Some(c) => match c.icc() {
            None => w.block("PDF/A embeds the output intent's ICC profile: choose a profile file", None, None),
            Some(icc) if !(icc.is_output() || icc.class == *b"mntr") => {
                w.block("The ICC profile {} isn't an output or display profile", Some(c.label()), None);
            }
            Some(icc) => match icc.colorants {
                None => w.block("The ICC profile {} isn't a gray, RGB or CMYK profile", Some(c.label()), None),
                Some(family) => {
                    let mut profile = None;
                    w.reuse_profile(c, &mut profile);
                    w.default_spaces(family, true);
                    w.set_intent("GTS_PDFA1", c, &mut profile);
                }
            },
        },
        None => {
            if used.cmyk && !used.default_cmyk && crate::pdfa_intent_profile(&w.doc).is_none() {
                w.block("The document uses CMYK colour: choose the ICC profile of a CMYK printing condition for its output intent", None, None);
            }
        }
    }
    w.inline_fonts();
    w.dates(opts);
    if level.needs_structure() {
        match crate::levels::mark_tagged(&mut w.doc) {
            Ok(true) => w.fixed.push("Marked the document as tagged (it has a structure tree)".into()),
            Ok(false) => {}
            Err(e) => w.block("The document can't be converted: {}", Some(e.to_string()), None),
        }
    }
    match crate::convert(&mut w.doc, level) {
        Ok(r) => w.fixed.extend(r.fixed),
        Err(e) => w.block("The document can't be converted: {}", Some(e.to_string()), None),
    }
    w.version("1.7");
    let issues = crate::verify(&w.doc, level);
    w.blockers.extend(issues.iter().map(Blocker::from));
    w.finish(doc)
}

/// The copy being converted, what has been done to it and what stands in the way.
struct Work {
    doc: Document,
    fixed: Vec<String>,
    blockers: Vec<Blocker>,
    /// The level's label, for messages.
    label: &'static str,
}

/// Rectangles may stray this far before they count as different (writers round).
const EPS: f64 = 0.01;

fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        &[a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
        _ => None,
    }
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some(r)
}

fn same(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() <= EPS)
}

fn rect_object(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| pdfcraft_content::num((v * 1000.0).round() / 1000.0)).collect())
}

/// A stream's filter names.
fn filter_names(doc: &Document, s: &Stream) -> Vec<Vec<u8>> {
    match s.dict.get(b"Filter").map(|f| doc.resolve(f)).as_deref() {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(|x| x.as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// A 16-byte identifier as a UUID URN (`uuid:…`).
fn uuid(id: &[u8]) -> String {
    let mut bytes = [0u8; 16];
    if id.len() == 16 {
        bytes.copy_from_slice(id);
    } else {
        bytes = hash16(id);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let part = |a: usize, b: usize| hex.get(a..b).unwrap_or_default().to_owned();
    format!("uuid:{}-{}-{}-{}-{}", part(0, 8), part(8, 12), part(12, 16), part(16, 20), part(20, 32))
}

/// FNV-1a, twice, over `data`: a stable 16-byte digest (not a secure one).
fn hash16(data: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (half, seed) in [0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4].into_iter().enumerate() {
        let mut h = seed;
        for b in data {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        if let Some(slot) = out.get_mut(half * 8..half * 8 + 8) {
            slot.copy_from_slice(&h.to_be_bytes());
        }
    }
    out
}

impl Work {
    fn block(&mut self, pattern: &'static str, value: Option<String>, page: Option<usize>) {
        self.blockers.push(Blocker::new(pattern, value, page));
    }

    /// Keep the converted copy, or refuse.
    fn finish(mut self, doc: &mut Document) -> Result<Vec<String>, Refusal> {
        let mut seen = HashSet::new();
        self.blockers.retain(|b| seen.insert((b.message.clone(), b.page)));
        if self.blockers.is_empty() {
            *doc = self.doc;
            Ok(self.fixed)
        } else {
            Err(Refusal { level: self.label, blockers: self.blockers })
        }
    }

    fn root(&self) -> Option<ObjRef> {
        self.doc.root()
    }

    fn update_catalog(&mut self, f: impl FnOnce(&mut Dict)) {
        let Some(root) = self.root() else {
            self.block("The document can't be converted: {}", Some("it has no catalog".into()), None);
            return;
        };
        if let Err(e) = self.doc.update_dict(root, f) {
            self.block("The document can't be converted: {}", Some(e.to_string()), None);
        }
    }

    /// Remove encryption the user may remove: a document opened with its owner (permissions)
    /// password, or encrypted with none.
    fn security(&mut self) {
        if self.doc.trailer().get(b"Encrypt").is_none() {
            return;
        }
        if self.doc.permissions().is_some_and(|p| p.owner) {
            self.doc.remove_encryption();
            self.fixed.push("Removed the security (encryption)".into());
        } else {
            self.block("The document is encrypted: open it with its permissions password to remove the security", None, None);
        }
    }

    /// Features of PDF versions after `version`.
    fn version_features(&mut self, version: &'static str) {
        let label = Some(self.label.to_owned());
        if version < "1.5" {
            if crate::catalog(&self.doc).get(b"OCProperties").is_some() {
                self.block("Layers (optional content) need PDF 1.5, newer than {} allows", label.clone(), None);
            }
            let jpx = crate::objects(&self.doc)
                .iter()
                .any(|(_, o)| matches!(&**o, Object::Stream(s) if filter_names(&self.doc, s).iter().any(|f| f == b"JPXDecode")));
            if jpx {
                self.block("JPEG 2000 pictures need PDF 1.5, newer than {} allows", label.clone(), None);
            }
        }
        if version < "1.6"
            && crate::objects(&self.doc).iter().any(|(_, o)| matches!(&**o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"OpenType")))
        {
            self.block("OpenType font programs need PDF 1.6, newer than {} allows", label, None);
        }
    }

    /// Fonts written into resource dictionaries rather than as objects of their own, which the
    /// verifiers don't reach: every font is embedded, as both standards require.
    fn inline_fonts(&mut self) {
        let mut found = Vec::new();
        for c in walk::contents(&self.doc) {
            for (_, f) in walk::dict_of(&self.doc, c.resources.get(b"Font")).iter() {
                if let Object::Dict(d) = f {
                    crate::check_font(&self.doc, d, &mut |_, pattern, value, _, _| found.push((pattern, value, c.page)));
                }
            }
        }
        for (pattern, value, page) in found {
            self.block(pattern, value, page);
        }
    }

    /// No JavaScript and nothing that runs by itself (PDF/X).
    fn scripts(&mut self) {
        let mut removed = 0usize;
        let cat = crate::catalog(&self.doc);
        let javascript =
            |doc: &Document, o: &Object| doc.resolve(o).as_dict().is_some_and(|a| a.name(b"S") == Some(b"JavaScript") && crate::is_action(a));
        match cat.get(b"Names").cloned() {
            Some(Object::Ref(r)) => {
                let mut had = false;
                if self.doc.update_dict(r, |n| had = n.remove(b"JavaScript").is_some()).is_ok() && had {
                    removed += 1;
                }
            }
            Some(Object::Dict(n)) if n.get(b"JavaScript").is_some() => {
                self.update_catalog(|c| {
                    let mut n = n.clone();
                    n.remove(b"JavaScript");
                    c.set(b"Names".to_vec(), Object::Dict(n));
                });
                removed += 1;
            }
            _ => {}
        }
        let open = cat.get(b"OpenAction").is_some_and(|a| javascript(&self.doc, a));
        if open {
            self.update_catalog(|c| {
                c.remove(b"OpenAction");
            });
            removed += 1;
        }
        for (r, o) in crate::objects(&self.doc) {
            match &*o {
                Object::Dict(d) => {
                    if d.name(b"S") == Some(b"JavaScript") && crate::is_action(d) {
                        self.doc.set(r, Object::Null);
                        removed += 1;
                        continue;
                    }
                    let mut nd = d.clone();
                    let mut changed = nd.remove(b"AA").is_some();
                    if nd.get(b"A").is_some_and(|a| javascript(&self.doc, a)) {
                        nd.remove(b"A");
                        changed = true;
                    }
                    if changed {
                        self.doc.set(r, Object::Dict(nd));
                        removed += 1;
                    }
                }
                Object::Stream(s) if s.dict.get(b"AA").is_some() => {
                    let mut ns = s.clone();
                    ns.dict.remove(b"AA");
                    self.doc.set(r, Object::Stream(ns));
                    removed += 1;
                }
                _ => {}
            }
        }
        if removed > 0 {
            self.fixed.push(format!("{removed}: removed JavaScript or additional actions"));
        }
    }

    /// LZW re-encoded with Flate; OPI comments and reference-XObject links dropped.
    fn streams(&mut self) {
        let (mut lzw, mut opi, mut refs) = (0usize, 0usize, 0usize);
        for (r, o) in crate::objects(&self.doc) {
            let Object::Stream(s) = &*o else { continue };
            let mut ns = s.clone();
            let mut changed = false;
            if filter_names(&self.doc, s).iter().any(|f| f == b"LZWDecode" || f == b"LZW")
                && let Ok(data) = s.decoded_strict()
            {
                let mut dict = s.dict.clone();
                dict.remove(b"Length");
                ns = Stream::flate(dict, &data);
                lzw += 1;
                changed = true;
            }
            let sub = s.dict.name(b"Subtype");
            if matches!(sub, Some(b"Image" | b"Form")) && ns.dict.remove(b"OPI").is_some() {
                opi += 1;
                changed = true;
            }
            if sub == Some(b"Form") && ns.dict.remove(b"Ref").is_some() {
                refs += 1;
                changed = true;
            }
            if changed {
                self.doc.set(r, Object::Stream(ns));
            }
        }
        for (n, what) in [
            (lzw, "re-encoded LZW streams with Flate"),
            (opi, "removed OPI comments (the pictures in the file stay)"),
            (refs, "removed reference-XObject links (the proxies in the file stay)"),
        ] {
            if n > 0 {
                self.fixed.push(format!("{n}: {what}"));
            }
        }
    }

    /// A TrimBox and a BleedBox on every page, nested in the CropBox and the MediaBox.
    fn page_boxes(&mut self) {
        let (mut trims, mut bleeds, mut arts, mut clamped) = (0usize, 0usize, 0usize, 0usize);
        for (i, p) in pdfcraft_model::pages(&self.doc).iter().enumerate() {
            let boxed = |key: &[u8]| rect(&self.doc, p.dict.get(key));
            let Some(media) = boxed(b"MediaBox") else { continue };
            let crop = boxed(b"CropBox").and_then(|c| intersect(c, media)).unwrap_or(media);
            let (mut trim, mut art) = (boxed(b"TrimBox"), boxed(b"ArtBox"));
            let mut set: Vec<(&[u8], Option<[f64; 4]>)> = Vec::new();
            if trim.is_some() && art.is_some() {
                set.push((b"ArtBox", None));
                art = None;
                arts += 1;
            }
            if trim.is_none() && art.is_none() {
                trim = Some(crop);
                set.push((b"TrimBox", Some(crop)));
                trims += 1;
            }
            let bleed = match boxed(b"BleedBox") {
                Some(b) => b,
                None => {
                    set.push((b"BleedBox", Some(crop)));
                    bleeds += 1;
                    crop
                }
            };
            let fitted_bleed = intersect(bleed, media);
            if let Some(b) = fitted_bleed
                && !same(b, bleed)
            {
                set.push((b"BleedBox", Some(b)));
                clamped += 1;
            }
            let (key, finished): (&[u8], Option<[f64; 4]>) = match (trim, art) {
                (Some(t), _) => (b"TrimBox", Some(t)),
                (None, a) => (b"ArtBox", a),
            };
            if let Some(f) = finished {
                match fitted_bleed.and_then(|b| intersect(f, b)).and_then(|f| intersect(f, crop)) {
                    Some(fitted) if !same(fitted, f) => {
                        set.push((key, Some(fitted)));
                        clamped += 1;
                    }
                    Some(_) => {}
                    None => self.block("Page {}'s TrimBox, BleedBox and CropBox don't overlap", Some((i + 1).to_string()), Some(i)),
                }
            }
            if set.is_empty() {
                continue;
            }
            let result = self.doc.update_dict(p.obj, |d| {
                for (k, v) in set {
                    match v {
                        Some(r) => d.set(k.to_vec(), rect_object(r)),
                        None => {
                            d.remove(k);
                        }
                    }
                }
            });
            if let Err(e) = result {
                self.block("The document can't be converted: {}", Some(e.to_string()), Some(i));
            }
        }
        for (n, what) in [
            (trims, "set the TrimBox to the CropBox"),
            (bleeds, "set the BleedBox to the CropBox"),
            (arts, "removed an ArtBox beside the TrimBox"),
            (clamped, "fitted a box inside the boxes around it"),
        ] {
            if n > 0 {
                self.fixed.push(format!("{n} pages: {what}"));
            }
        }
    }

    /// Annotations in the bleed that print nothing: links, and hidden or non-printing
    /// annotations (form fields stay: they belong to the form).
    fn unprinted_annotations(&mut self) {
        let mut removed = 0usize;
        for p in pdfcraft_model::pages(&self.doc) {
            let boxed = |key: &[u8]| rect(&self.doc, p.dict.get(key));
            let Some(media) = boxed(b"MediaBox") else { continue };
            let crop = boxed(b"CropBox").unwrap_or(media);
            let bleed = boxed(b"BleedBox").unwrap_or(crop);
            let entry = p.dict.get(b"Annots").cloned();
            let annots = entry.as_ref().map(|a| self.doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
            let mut gone: Vec<ObjRef> = Vec::new();
            let unprinted = |d: &Dict| {
                let f = d.int(b"F").unwrap_or(0);
                d.name(b"Subtype") == Some(b"Link") || f & 4 == 0 || f & 2 != 0
            };
            let kept: Vec<Object> = annots
                .iter()
                .filter(|a| {
                    let d = self.doc.resolve(a);
                    let Some(d) = d.as_dict() else { return true };
                    let in_bleed = rect(&self.doc, d.get(b"Rect")).is_some_and(|r| intersect(r, bleed).is_some());
                    let drop = in_bleed && !matches!(d.name(b"Subtype"), Some(b"Widget" | b"Popup")) && unprinted(d);
                    if drop && let Some(r) = a.as_ref() {
                        gone.push(r);
                    }
                    !drop
                })
                .cloned()
                .collect();
            if kept.len() == annots.len() {
                continue;
            }
            // Pop-ups of what went go too.
            let kept: Vec<Object> = kept
                .into_iter()
                .filter(|a| {
                    let d = self.doc.resolve(a);
                    !d.as_dict().is_some_and(|d| {
                        d.name(b"Subtype") == Some(b"Popup") && d.get(b"Parent").and_then(Object::as_ref).is_some_and(|r| gone.contains(&r))
                    })
                })
                .collect();
            removed += annots.len().saturating_sub(kept.len());
            let result = match entry {
                Some(Object::Ref(r)) if matches!(&*self.doc.get(r), Object::Array(_)) => {
                    self.doc.set(r, Object::Array(kept));
                    Ok(())
                }
                _ => self.doc.update_dict(p.obj, |d| d.set(b"Annots".to_vec(), Object::Array(kept))),
            };
            if let Err(e) = result {
                self.block("The document can't be converted: {}", Some(e.to_string()), None);
            }
        }
        if removed > 0 {
            self.fixed.push(format!("{removed}: removed links and annotations that don't print from the bleed"));
        }
    }

    /// When an output intent already embeds the same profile, use that object (ISO 19005-2
    /// 6.2.3: intents with profiles share one).
    fn reuse_profile(&self, c: &OutputCondition, profile: &mut Option<ObjRef>) {
        let Some(icc) = c.icc() else { return };
        let cat = crate::catalog(&self.doc);
        let list = cat.get(b"OutputIntents").map(|o| self.doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        *profile = list.iter().find_map(|oi| {
            let r = self.doc.resolve(oi).as_dict()?.get(b"DestOutputProfile")?.as_ref()?;
            match &*self.doc.get(r) {
                Object::Stream(s) if s.decoded_strict().ok().as_deref() == Some(icc.bytes.as_slice()) => Some(r),
                _ => None,
            }
        });
    }

    /// The output intent of type `subtype`, replacing any the document has.
    fn set_intent(&mut self, subtype: &str, c: &OutputCondition, profile: &mut Option<ObjRef>) {
        let cat = crate::catalog(&self.doc);
        let list = cat.get(b"OutputIntents").map(|o| self.doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        let mut kept: Vec<Object> = list
            .into_iter()
            .filter(|oi| {
                self.doc
                    .resolve(oi)
                    .as_dict()
                    .is_none_or(|d| d.get(b"S").map(|s| self.doc.resolve(s)).as_deref().and_then(Object::as_name) != Some(subtype.as_bytes()))
            })
            .collect();
        kept.push(Object::Dict(crate::output::intent(&mut self.doc, subtype, c, profile)));
        self.update_catalog(|cat| cat.set(b"OutputIntents".to_vec(), Object::Array(kept)));
        self.fixed.push(format!("Set the output intent ({subtype}) to {}", c.label()));
    }

    /// PDF/X-1a: colour to CMYK through the output profile.
    fn convert_to_cmyk(&mut self, icc: &[u8], label: &str) {
        let r = crate::recolour::to_cmyk(&mut self.doc, icc);
        self.blockers.extend(r.blockers);
        let s = r.stats;
        if s.colours + s.pictures + s.tables > 0 {
            self.fixed
                .push(format!("Converted {} colours, {} pictures and {} colour tables to CMYK through {label}", s.colours, s.pictures, s.tables));
        }
        if s.spaces_removed > 0 {
            self.fixed.push(format!("{}: removed colour spaces content no longer uses", s.spaces_removed));
        }
        if s.group_spaces > 0 {
            self.fixed.push(format!("{}: removed the RGB blending space of a transparency group", s.group_spaces));
        }
    }

    /// Device colour the printing condition's colour space (`family`) doesn't hold, mapped by a
    /// `Default` space: RGB to sRGB, and gray to a gray profile under an RGB condition. CMYK can't
    /// be mapped. `independent`: whether the output intent can describe device-independent colour
    /// (it embeds a profile).
    fn default_spaces(&mut self, family: Colorants, independent: bool) {
        let used = crate::uses_device_colour(&self.doc, &crate::objects(&self.doc));
        if used.cmyk && !used.default_cmyk && family != Colorants::Cmyk {
            self.block("CMYK colour needs a CMYK printing condition", None, None);
        }
        let rgb = used.rgb && family != Colorants::Rgb;
        let gray = used.gray && family == Colorants::Rgb;
        if !(rgb || gray) {
            return;
        }
        if !independent {
            self.block(
                "{} needs the printing condition's ICC profile embedded for RGB or device-independent colour: choose a profile file",
                Some(self.label.to_owned()),
                None,
            );
            return;
        }
        /// A `Default` space: its key, the device space it maps, the profile and its components.
        struct Mapping {
            wanted: bool,
            key: &'static [u8],
            device: &'static [u8],
            profile: fn() -> Vec<u8>,
            n: i64,
            what: &'static str,
        }
        let mappings = [
            Mapping { wanted: rgb, key: b"DefaultRGB", device: b"DeviceRGB", profile: crate::icc::srgb, n: 3, what: "RGB colour is now sRGB" },
            Mapping {
                wanted: gray,
                key: b"DefaultGray",
                device: b"DeviceGray",
                profile: crate::icc::gray,
                n: 1,
                what: "Gray colour now has a gray profile",
            },
        ];
        for Mapping { wanted, key, device, profile, n, what } in mappings {
            if !wanted {
                continue;
            }
            let mut d = Dict::new();
            d.set(b"N".to_vec(), Object::Int(n));
            let stream = self.doc.add(Object::Stream(Stream::flate(d, &profile())));
            let space = Object::Array(vec![Object::name("ICCBased"), Object::Ref(stream)]);
            match add_default(&mut self.doc, key, device, &space) {
                Ok(count) => self.fixed.push(format!("{what} ({}, in {count} places)", String::from_utf8_lossy(key))),
                Err(e) => self.block("The document can't be converted: {}", Some(e.to_string()), None),
            }
        }
    }

    /// The document information's PDF/X entries, title and dates. Returns what `Trapped` says.
    fn pdfx_information(&mut self, level: pdfx::Level, opts: &Options) -> bool {
        let trapped = match pdfx_info(&self.doc, b"Trapped").as_deref() {
            Some(Object::Name(n)) if n == b"True" => true,
            Some(Object::Bool(true)) => true,
            Some(Object::Name(n)) if n == b"False" => false,
            _ => {
                self.fixed.push("Trapped set to /False: the document didn't say it was trapped".into());
                false
            }
        };
        let mut set: Vec<(&[u8], Object)> = vec![
            (b"GTS_PDFXVersion", Object::String(PdfString::text(level.version_key()))),
            (b"Trapped", Object::name(if trapped { "True" } else { "False" })),
        ];
        if level == pdfx::Level::X1a2001 {
            set.push((b"GTS_PDFXConformance", Object::String(PdfString::text(level.label()))));
        }
        if crate::info(&self.doc, "Title").is_none_or(|t| t.trim().is_empty())
            && let Some(t) = &opts.title
        {
            set.push((b"Title", Object::String(PdfString::text(t))));
        }
        let remove: &[&[u8]] = if level == pdfx::Level::X1a2001 { &[] } else { &[b"GTS_PDFXConformance"] };
        self.information(&set, remove);
        self.dates(opts);
        self.fixed.push(format!("Identified the file as {} (GTS_PDFXVersion)", level.label()));
        trapped
    }

    /// The modification date now, and a creation date when there is none.
    fn dates(&mut self, opts: &Options) {
        let Some(now) = &opts.now else { return };
        let mut set: Vec<(&[u8], Object)> = vec![(b"ModDate", Object::String(PdfString::literal(now.as_bytes().to_vec())))];
        if crate::info(&self.doc, "CreationDate").is_none() {
            set.push((b"CreationDate", Object::String(PdfString::literal(now.as_bytes().to_vec()))));
        }
        self.information(&set, &[]);
    }

    /// Set and remove document information entries, creating the dictionary when needed.
    fn information(&mut self, set: &[(&[u8], Object)], remove: &[&[u8]]) {
        let apply = |d: &mut Dict| {
            for (k, v) in set {
                d.set(k.to_vec(), v.clone());
            }
            for k in remove {
                d.remove(k);
            }
        };
        match self.doc.trailer().get(b"Info").cloned() {
            Some(Object::Ref(r)) if matches!(&*self.doc.get(r), Object::Dict(_)) => {
                if let Err(e) = self.doc.update_dict(r, apply) {
                    self.block("The document can't be converted: {}", Some(e.to_string()), None);
                }
            }
            Some(Object::Dict(mut d)) => {
                apply(&mut d);
                let r = self.doc.add(Object::Dict(d));
                self.doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
            }
            _ => {
                let mut d = Dict::new();
                apply(&mut d);
                let r = self.doc.add(Object::Dict(d));
                self.doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
            }
        }
    }

    /// A document ID in the trailer (CGATS notes 2.22).
    fn trailer_id(&mut self, opts: &Options) {
        let ok = self
            .doc
            .trailer()
            .get(b"ID")
            .map(|o| self.doc.resolve(o))
            .as_deref()
            .and_then(Object::as_array)
            .is_some_and(|a| a.len() == 2 && a.iter().all(|s| self.doc.resolve(s).as_string().is_some()));
        if ok {
            return;
        }
        let data = self.doc.bytes();
        let mut seed: Vec<u8> = data.iter().step_by((data.len() / 4096).max(1)).copied().collect();
        seed.extend(opts.now.as_deref().unwrap_or_default().as_bytes());
        let id = Object::String(PdfString { bytes: hash16(&seed).to_vec(), hex: true });
        self.doc.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![id.clone(), id]));
        self.fixed.push("Gave the file a document ID".into());
    }

    /// The XMP metadata: the document information, the PDF/X identification (PDF/X-4), Trapped
    /// and the document's identity, keeping the PDF/UA identification and a PDF/A one the
    /// converted file still meets.
    fn pdfx_metadata(&mut self, level: pdfx::Level, trapped: bool, opts: &Options) {
        let old = crate::metadata(&self.doc);
        let pdfua = old.as_deref().and_then(|x| xmp::value(x, "pdfuaid:part")).and_then(|p| p.trim().parse().ok());
        let pdfa = crate::declared(&self.doc).pdfa.and_then(|(part, conformance)| Level::from_id(&format!("{part}{conformance}")));
        let id = self
            .doc
            .trailer()
            .get(b"ID")
            .map(|o| self.doc.resolve(o))
            .and_then(|a| a.as_array().and_then(|a| a.first()).map(|s| self.doc.resolve(s)))
            .and_then(|s| s.as_string().map(|s| s.bytes.clone()))
            .unwrap_or_default();
        let mut ids = xmp::Ids {
            pdfa: pdfa.map(|l| (l.part(), l.conformance())),
            pdfua,
            pdfx: level.is_x4().then_some(level.version_key()),
            trapped: Some(trapped),
            document_id: Some(uuid(&id)),
            now: opts.now.clone(),
        };
        self.write_metadata(&xmp::packet_with(&self.doc, &ids));
        if let Some(l) = pdfa
            && !crate::verify(&self.doc, l).is_empty()
        {
            ids.pdfa = None;
            self.write_metadata(&xmp::packet_with(&self.doc, &ids));
            self.fixed.push(format!("Dropped the {} identification: the converted file doesn't meet it", l.label()));
        }
        self.fixed.push("Wrote the XMP metadata".into());
    }

    fn write_metadata(&mut self, packet: &str) {
        let mut md = Dict::new();
        md.set(b"Type".to_vec(), Object::name("Metadata"));
        md.set(b"Subtype".to_vec(), Object::name("XML"));
        // Uncompressed, so other tools can find it.
        let stream = Object::Stream(Stream::from_raw(md, packet.as_bytes().to_vec()));
        match crate::catalog(&self.doc).get(b"Metadata").and_then(Object::as_ref) {
            Some(r) => self.doc.set(r, stream),
            None => {
                let r = self.doc.add(stream);
                self.update_catalog(|c| c.set(b"Metadata".to_vec(), Object::Ref(r)));
            }
        }
    }

    /// A header no newer than `version`, and no catalog `/Version` newer either. A file with
    /// cross-reference streams is rewritten in full before PDF 1.5.
    fn version(&mut self, version: &'static str) {
        let current = self.doc.version().chars().take(3).collect::<String>();
        let mut full = false;
        if current.as_str() > version {
            self.doc.set_version(version);
            full = true;
            self.fixed.push(format!("Wrote the file as PDF {version}"));
        }
        if crate::catalog(&self.doc).name(b"Version").is_some_and(|v| v > version.as_bytes()) {
            self.update_catalog(|c| {
                c.remove(b"Version");
            });
            full = true;
        }
        if version < "1.5" && self.doc.revisions().iter().any(|r| r.is_stream) {
            full = true;
        }
        if full {
            self.doc.require_full_save();
        }
    }

    /// PDF/X-4: each optional-content configuration has a `Name` and no `AS`.
    fn optional_content(&mut self) {
        let cat = crate::catalog(&self.doc);
        let Some(entry) = cat.get(b"OCProperties").cloned() else { return };
        let mut changed = 0usize;
        let fix = |d: &mut Dict, name: &str, changed: &mut usize| {
            if d.get(b"Name").is_none() {
                d.set(b"Name".to_vec(), PdfString::text(name));
                *changed += 1;
            }
            if d.remove(b"AS").is_some() {
                *changed += 1;
            }
        };
        let mut props = walk::dict_of(&self.doc, Some(&entry));
        let mut configs: Vec<(Option<ObjRef>, String)> = Vec::new();
        if let Some(d) = props.get(b"D").cloned() {
            configs.push((d.as_ref(), "Default".into()));
        }
        let list = props.get(b"Configs").map(|c| self.doc.resolve(c)).and_then(|c| c.as_array().cloned()).unwrap_or_default();
        configs.extend(list.iter().enumerate().map(|(i, c)| (c.as_ref(), format!("Configuration {}", i + 1))));
        // Configurations that are objects of their own.
        for (r, name) in &configs {
            if let Some(r) = r
                && matches!(&*self.doc.get(*r), Object::Dict(_))
            {
                let mut n = 0;
                let _ = self.doc.update_dict(*r, |d| fix(d, name, &mut n));
                changed += n;
            }
        }
        // Configurations written into the properties.
        if let Some(Object::Dict(mut d)) = props.get(b"D").cloned() {
            fix(&mut d, "Default", &mut changed);
            props.set(b"D".to_vec(), Object::Dict(d));
        }
        if let Some(Object::Array(list)) = props.get(b"Configs").cloned() {
            let list: Vec<Object> = list
                .into_iter()
                .enumerate()
                .map(|(i, c)| match c {
                    Object::Dict(mut d) => {
                        fix(&mut d, &format!("Configuration {}", i + 1), &mut changed);
                        Object::Dict(d)
                    }
                    other => other,
                })
                .collect();
            props.set(b"Configs".to_vec(), Object::Array(list));
        }
        if changed == 0 {
            return;
        }
        match entry {
            Object::Ref(r) => self.doc.set(r, Object::Dict(props)),
            _ => self.update_catalog(|c| c.set(b"OCProperties".to_vec(), Object::Dict(props))),
        }
        self.fixed.push("Named the layer configurations and removed their automatic states (AS)".into());
    }
}

/// A document information entry, resolved.
fn pdfx_info(doc: &Document, key: &[u8]) -> Option<std::sync::Arc<Object>> {
    let info = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    info.as_dict()?.get(key).map(|v| doc.resolve(v))
}

/// Add `key` (`DefaultRGB`) = `space` to every resource dictionary that lacks it, give resources
/// to pages and appearance streams that have none, and point pictures in `device` at `space`
/// (soft masks stay DeviceGray, as they must). Returns how many places changed.
fn add_default(doc: &mut Document, key: &[u8], device: &[u8], space: &Object) -> Result<usize, pdfcraft_cos::CosError> {
    let mut count = 0usize;
    let mut resources: Vec<ObjRef> = Vec::new();
    let mut lists: Vec<ObjRef> = Vec::new();
    let mut masks: HashSet<ObjRef> = HashSet::new();
    let objs = crate::objects(doc);
    for (_, o) in &objs {
        if let Some(d) = crate::dict_of(o)
            && let Some(m) = d.get(b"SMask").and_then(Object::as_ref)
        {
            masks.insert(m);
        }
    }
    // Adds the entry to a resource dictionary's colour-space list; a list of its own is noted.
    let into = |res: &mut Dict, lists: &mut Vec<ObjRef>| -> bool {
        match res.get(b"ColorSpace").cloned() {
            Some(Object::Ref(r)) => {
                lists.push(r);
                false
            }
            Some(Object::Dict(mut list)) => {
                if list.contains(key) {
                    return false;
                }
                list.set(key.to_vec(), space.clone());
                res.set(b"ColorSpace".to_vec(), Object::Dict(list));
                true
            }
            None => {
                let mut list = Dict::new();
                list.set(key.to_vec(), space.clone());
                res.set(b"ColorSpace".to_vec(), Object::Dict(list));
                true
            }
            _ => false,
        }
    };
    for (r, o) in &objs {
        let Some(d) = crate::dict_of(o) else { continue };
        // Pictures in the device space.
        if d.name(b"Subtype") == Some(b"Image") && d.name(b"ColorSpace") == Some(device) && !masks.contains(r) {
            if let Object::Stream(s) = &**o {
                let mut ns = s.clone();
                ns.dict.set(b"ColorSpace".to_vec(), space.clone());
                doc.set(*r, Object::Stream(ns));
                count += 1;
            }
            continue;
        }
        match d.get(b"Resources") {
            Some(Object::Ref(res)) => resources.push(*res),
            Some(Object::Dict(res)) => {
                let mut res = res.clone();
                if into(&mut res, &mut lists) {
                    match &**o {
                        Object::Stream(s) => {
                            let mut ns = s.clone();
                            ns.dict.set(b"Resources".to_vec(), Object::Dict(res));
                            doc.set(*r, Object::Stream(ns));
                        }
                        _ => doc.update_dict(*r, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?,
                    }
                    count += 1;
                }
            }
            _ => {}
        }
    }
    // Pages with no resources, even inherited ones, and appearances with none: device colour
    // there would otherwise stay unmapped.
    let fresh = || {
        let mut list = Dict::new();
        list.set(key.to_vec(), space.clone());
        let mut res = Dict::new();
        res.set(b"ColorSpace".to_vec(), Object::Dict(list));
        Object::Dict(res)
    };
    for p in pdfcraft_model::pages(doc) {
        if p.dict.get(b"Resources").is_none() {
            doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), fresh()))?;
            count += 1;
        }
        for a in walk::appearances(doc, &p.dict) {
            if let Object::Stream(s) = &*doc.get(a)
                && s.dict.get(b"Resources").is_none()
            {
                let mut ns = s.clone();
                ns.dict.set(b"Resources".to_vec(), fresh());
                doc.set(a, Object::Stream(ns));
                count += 1;
            }
        }
    }
    resources.sort_unstable();
    resources.dedup();
    for r in resources {
        if let Object::Dict(mut res) = (*doc.get(r)).clone()
            && into(&mut res, &mut lists)
        {
            doc.set(r, Object::Dict(res));
            count += 1;
        }
    }
    lists.sort_unstable();
    lists.dedup();
    for r in lists {
        if let Object::Dict(mut list) = (*doc.get(r)).clone()
            && !list.contains(key)
        {
            list.set(key.to_vec(), space.clone());
            doc.set(r, Object::Dict(list));
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
