//! Preflight: run a profile — a named set of checks, each with a severity — over a document, and
//! get back the results grouped by check, each hit with its page, object and box.
//!
//! - A [`Profile`] is data (serde): its checks and their parameters can be read from JSON, so a
//!   custom profile is a file away. [`builtin`] lists the profiles PdfCraft ships and
//!   [`single_checks`] one profile per print check, for running a check on its own.
//! - The checks wrap the verifiers this crate already has (PDF/A, PDF/X), add what a print shop
//!   looks at (fonts, colour, spot colours, image resolution, hairlines, transparency, page boxes,
//!   encryption, JavaScript, annotations in the trim, the PDF version, `Trapped`), and run the
//!   Accessibility Checker through the [`Host`], which lives in a crate above this one.
//! - [`run`] changes nothing. A hit may name a [`Fixup`] that addresses it; [`fix`] applies one,
//!   and the engine makes that an undoable edit.
//! - [`text`] and [`json`] write a report; the engine turns the text into a PDF.
//!
//! Every sentence is worded here with named holes (`{}` or `{name}`) where the document's own
//! names go, and listed in [`WORDINGS`] or [`REPORT_WORDS`], so an interface can say it in its own
//! language with [`Hit::said`].

use std::collections::HashSet;

use pdfcraft_cos::Document;
use serde::{Deserialize, Serialize};

use crate::Words;

mod checks;
mod fixups;
mod report;
mod walk;

pub use fixups::{FIXUPS, Fixup, fix};
pub use report::{REPORT_WORDS, json, text};

/// How much a check's hits matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The file can't be used as it is.
    Error,
    /// The file can be used, but someone should look.
    Warning,
    /// Listed for information (spot colours, the PDF version).
    Info,
}

impl Severity {
    /// Most severe first.
    pub const ALL: [Severity; 3] = [Severity::Error, Severity::Warning, Severity::Info];

    pub fn id(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }

    pub fn from_id(id: &str) -> Option<Severity> {
        Severity::ALL.into_iter().find(|s| s.id() == id)
    }

    /// What a group of results of this severity is called.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Errors",
            Severity::Warning => "Warnings",
            Severity::Info => "Information",
        }
    }
}

/// A colour family that a [`Check::ColourSpaces`] check can refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    /// DeviceGray, CalGray and ICC-based gray.
    Gray,
    /// DeviceRGB, CalRGB and ICC-based RGB.
    Rgb,
    /// DeviceCMYK and ICC-based CMYK.
    Cmyk,
    Lab,
    /// Any ICC-based or calibrated (CalGray, CalRGB) space.
    Icc,
    /// Separation and DeviceN spaces that name a colorant other than the process inks.
    Spot,
}

impl Space {
    pub const ALL: [Space; 6] = [Space::Gray, Space::Rgb, Space::Cmyk, Space::Lab, Space::Icc, Space::Spot];

    pub fn id(self) -> &'static str {
        match self {
            Space::Gray => "gray",
            Space::Rgb => "rgb",
            Space::Cmyk => "cmyk",
            Space::Lab => "lab",
            Space::Icc => "icc",
            Space::Spot => "spot",
        }
    }
}

/// One check and its parameters. Profiles hold these as data: in JSON, `{"check":
/// "image-resolution", "colour_ppi": 300, "gray_ppi": 300, "bitmap_ppi": 1200}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "kebab-case")]
pub enum Check {
    /// Fonts whose program the file doesn't carry.
    FontsEmbedded,
    /// Colour in a family the job can't take, such as RGB or Lab in a CMYK job: content, images
    /// and shadings the pages draw.
    ColourSpaces { forbid: Vec<Space> },
    /// The spot colours the pages use, listed; with `max`, more than that many is a hit too.
    SpotColours {
        #[serde(default)]
        max: Option<usize>,
    },
    /// Images drawn below these effective resolutions (pixels per inch where they are placed).
    /// A bitmap is a one-bit image or an image mask.
    ImageResolution { colour_ppi: f64, gray_ppi: f64, bitmap_ppi: f64 },
    /// Strokes thinner than this, in points where they are drawn (a width of 0 is the thinnest
    /// line the device can draw). Reported only.
    Hairlines { min_width_pt: f64 },
    /// Soft masks, opacity, blend modes and transparency groups that pages use.
    Transparency,
    /// Pages with neither a TrimBox nor an ArtBox.
    TrimBox,
    /// Pages without a BleedBox, or whose bleed past the trim is narrower than this.
    Bleed { min_mm: f64 },
    /// An encrypted file.
    Encryption,
    /// Document JavaScript, JavaScript and Launch actions, and additional actions.
    Scripts,
    /// Annotations (other than pop-ups) that lie inside the trim.
    AnnotationsInTrim,
    /// The PDF version; with `max`, a newer version is a hit, and without it the version is listed.
    Version {
        #[serde(default)]
        max: Option<String>,
    },
    /// The document information's `Trapped` key: missing, not `/True` or `/False`, or not what the
    /// metadata says.
    Trapped,
    /// Every rule of a PDF/A level ([`crate::verify`]).
    #[serde(rename = "pdfa")]
    PdfA {
        #[serde(with = "pdfa_level")]
        level: crate::Level,
    },
    /// Every rule of a PDF/X level ([`crate::pdfx::verify`]).
    #[serde(rename = "pdfx")]
    PdfX {
        #[serde(with = "pdfx_level")]
        level: crate::pdfx::Level,
    },
    /// The Accessibility Checker's rules that are on by default, through the [`Host`].
    Accessibility,
}

impl Check {
    /// A stable id, the JSON `check` tag.
    pub fn id(&self) -> &'static str {
        match self {
            Check::FontsEmbedded => "fonts-embedded",
            Check::ColourSpaces { .. } => "colour-spaces",
            Check::SpotColours { .. } => "spot-colours",
            Check::ImageResolution { .. } => "image-resolution",
            Check::Hairlines { .. } => "hairlines",
            Check::Transparency => "transparency",
            Check::TrimBox => "trim-box",
            Check::Bleed { .. } => "bleed",
            Check::Encryption => "encryption",
            Check::Scripts => "scripts",
            Check::AnnotationsInTrim => "annotations-in-trim",
            Check::Version { .. } => "version",
            Check::Trapped => "trapped",
            Check::PdfA { .. } => "pdfa",
            Check::PdfX { .. } => "pdfx",
            Check::Accessibility => "accessibility",
        }
    }

    /// The check's name as a wording with an optional `{}` hole, and what goes in it (a standard's
    /// name, which reads the same in every language).
    pub fn title(&self) -> (&'static str, Option<String>) {
        let plain = |s: &'static str| (s, None);
        match self {
            Check::FontsEmbedded => plain("Fonts not embedded"),
            Check::ColourSpaces { .. } => plain("Colour the profile doesn't allow"),
            Check::SpotColours { .. } => plain("Spot colours"),
            Check::ImageResolution { .. } => plain("Image resolution"),
            Check::Hairlines { .. } => plain("Hairlines"),
            Check::Transparency => plain("Transparency"),
            Check::TrimBox => plain("Pages without a TrimBox"),
            Check::Bleed { .. } => plain("Bleed"),
            Check::Encryption => plain("Encryption"),
            Check::Scripts => plain("JavaScript and actions"),
            Check::AnnotationsInTrim => plain("Annotations inside the trim"),
            Check::Version { .. } => plain("PDF version"),
            Check::Trapped => plain("Trapped key"),
            Check::PdfA { level } => ("{} compliance", Some(level.label().to_owned())),
            Check::PdfX { level } => ("{} compliance", Some(level.label().to_owned())),
            Check::Accessibility => plain("Accessibility Checker rules"),
        }
    }

    /// The check's name, said in the language `words` translates into.
    pub fn said(&self, words: Words) -> String {
        let (pattern, value) = self.title();
        let said = words(pattern);
        match value {
            Some(v) => said.replacen("{}", &v, 1),
            None => said,
        }
    }

    /// The parameters, each as a wording for its name and its value (numbers and PDF names, which
    /// read the same in every language).
    pub fn params(&self) -> Vec<(&'static str, String)> {
        match self {
            Check::ColourSpaces { forbid } => vec![("Not allowed", forbid.iter().map(|s| s.id()).collect::<Vec<_>>().join(", "))],
            Check::SpotColours { max: Some(m) } => vec![("Most spot colours", m.to_string())],
            Check::ImageResolution { colour_ppi, gray_ppi, bitmap_ppi } => vec![
                ("Colour images", format!("{} ppi", number(*colour_ppi))),
                ("Gray images", format!("{} ppi", number(*gray_ppi))),
                ("Bitmap images", format!("{} ppi", number(*bitmap_ppi))),
            ],
            Check::Hairlines { min_width_pt } => vec![("Thinnest line", format!("{} pt", number(*min_width_pt)))],
            Check::Bleed { min_mm } => vec![("Least bleed", format!("{} mm", number(*min_mm)))],
            Check::Version { max: Some(m) } => vec![("Highest version", m.clone())],
            _ => Vec::new(),
        }
    }

    /// Whether the check reads page content (so [`run`] walks the pages once for all of them).
    fn walks(&self) -> bool {
        matches!(
            self,
            Check::FontsEmbedded
                | Check::ColourSpaces { .. }
                | Check::SpotColours { .. }
                | Check::ImageResolution { .. }
                | Check::Hairlines { .. }
                | Check::Transparency
        )
    }

    /// Parameters a profile from outside may get wrong.
    fn invalid(&self) -> Option<String> {
        let bad = |v: f64| !v.is_finite() || v < 0.0;
        match self {
            Check::ImageResolution { colour_ppi, gray_ppi, bitmap_ppi } if [*colour_ppi, *gray_ppi, *bitmap_ppi].into_iter().any(bad) => {
                Some("image-resolution: resolutions must be finite and not negative".into())
            }
            Check::Hairlines { min_width_pt } if bad(*min_width_pt) || *min_width_pt > MAX_HAIRLINE_PT => {
                Some(format!("hairlines: min_width_pt must be between 0 and {MAX_HAIRLINE_PT}"))
            }
            Check::Bleed { min_mm } if bad(*min_mm) => Some("bleed: min_mm must be finite and not negative".into()),
            Check::Version { max: Some(m) } if parse_version(m).is_none() => Some(format!("version: {m:?} is not a PDF version such as 1.6")),
            _ => None,
        }
    }
}

/// The widest line a hairline check may ask about: past this a "hairline" is an ordinary rule,
/// and the walk would record every stroke on the page.
pub const MAX_HAIRLINE_PT: f64 = 4.0;

/// A number as a report shows it: no trailing zeros.
fn number(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_owned() }
}

/// `"1.7"` → (1, 7).
fn parse_version(v: &str) -> Option<(u32, u32)> {
    let (major, minor) = v.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

mod pdfa_level {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(level: &crate::Level, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(level.label())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<crate::Level, D::Error> {
        let id = String::deserialize(d)?;
        crate::Level::from_id(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown PDF/A level {id:?} (PDF/A-2b or PDF/A-3b)")))
    }
}

mod pdfx_level {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(level: &crate::pdfx::Level, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(level.label())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<crate::pdfx::Level, D::Error> {
        let id = String::deserialize(d)?;
        crate::pdfx::Level::from_id(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown PDF/X level {id:?}")))
    }
}

/// A check in a profile, and how much its hits matter there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckSpec {
    #[serde(flatten)]
    pub check: Check,
    pub severity: Severity,
}

/// A named set of checks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// A stable id, such as `commercial-print`.
    pub id: String,
    /// The name, in English. A built-in profile's name is one of [`WORDINGS`] (or a standard's
    /// name, which reads the same everywhere); a custom profile's is shown as it is written.
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub checks: Vec<CheckSpec>,
}

/// The most checks a profile may hold.
pub const MAX_CHECKS: usize = 256;

impl Profile {
    /// What is wrong with a profile from outside, if anything.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err("a profile needs an id and a name".into());
        }
        if self.checks.is_empty() {
            return Err("a profile needs at least one check".into());
        }
        if self.checks.len() > MAX_CHECKS {
            return Err(format!("a profile may hold at most {MAX_CHECKS} checks"));
        }
        match self.checks.iter().find_map(|c| c.check.invalid()) {
            Some(problem) => Err(problem),
            None => Ok(()),
        }
    }

    /// Read a profile from JSON, and check it.
    pub fn from_json(json: &str) -> Result<Profile, String> {
        let profile: Profile = serde_json::from_str(json).map_err(|e| format!("not a preflight profile: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }

    /// The profile as JSON.
    pub fn to_json(&self) -> String {
        // A profile is plain data (strings, numbers, enums): serializing it can't fail.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

fn spec(check: Check, severity: Severity) -> CheckSpec {
    CheckSpec { check, severity }
}

/// What a standard's profile says it does.
const VERIFIES: &str = "Verifies the file against the standard, without changing it";

/// The profiles PdfCraft ships, in the order a picker lists them.
pub fn builtin() -> Vec<Profile> {
    use Severity::{Error, Info, Warning};
    let mut out = vec![Profile {
        id: "commercial-print".into(),
        name: "Commercial print (CMYK)".into(),
        description: "Checks a file for CMYK printing: fonts, colour, image resolution, hairlines, transparency, page boxes, encryption, JavaScript, annotations, Trapped and the PDF version".into(),
        checks: vec![
            spec(Check::FontsEmbedded, Error),
            spec(Check::ColourSpaces { forbid: vec![Space::Rgb, Space::Lab] }, Error),
            spec(Check::Encryption, Error),
            spec(Check::ImageResolution { colour_ppi: 300.0, gray_ppi: 300.0, bitmap_ppi: 1200.0 }, Warning),
            spec(Check::Hairlines { min_width_pt: 0.25 }, Warning),
            spec(Check::Transparency, Warning),
            spec(Check::TrimBox, Warning),
            spec(Check::Bleed { min_mm: 3.0 }, Warning),
            spec(Check::Scripts, Warning),
            spec(Check::AnnotationsInTrim, Warning),
            spec(Check::SpotColours { max: None }, Info),
            spec(Check::Trapped, Info),
            spec(Check::Version { max: None }, Info),
        ],
    }];
    for level in crate::pdfx::Level::ALL {
        out.push(Profile {
            id: format!("pdfx-{}", level.label().trim_start_matches("PDF/X-").replace(':', "-").to_ascii_lowercase()),
            name: level.label().into(),
            description: VERIFIES.into(),
            checks: vec![spec(Check::PdfX { level }, Error)],
        });
    }
    for level in [crate::Level::A2b, crate::Level::A3b] {
        out.push(Profile {
            id: format!("pdfa-{}", level.label().trim_start_matches("PDF/A-")),
            name: level.label().into(),
            description: VERIFIES.into(),
            checks: vec![spec(Check::PdfA { level }, Error)],
        });
    }
    out.push(Profile {
        id: "accessibility".into(),
        name: "Accessibility".into(),
        description: "Runs the Accessibility Checker's rules".into(),
        checks: vec![spec(Check::Accessibility, Error)],
    });
    out
}

/// One profile per print check, with the commercial-print parameters and severity, for running a
/// check on its own. Their ids are `check:` and the check's id.
pub fn single_checks() -> Vec<Profile> {
    builtin()
        .into_iter()
        .find(|p| p.id == "commercial-print")
        .map(|p| p.checks)
        .unwrap_or_default()
        .into_iter()
        .map(|c| Profile { id: format!("check:{}", c.check.id()), name: c.check.title().0.into(), description: String::new(), checks: vec![c] })
        .collect()
}

/// A built-in profile or single check by id.
pub fn find(id: &str) -> Option<Profile> {
    builtin().into_iter().chain(single_checks()).find(|p| p.id == id)
}

/// What a check found: one problem, or one thing listed, with where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The sentence in English.
    pub message: String,
    /// The sentence as it is worded, with holes (`{}`, `{p}`, …) where `values` go.
    pub pattern: &'static str,
    /// What goes in each hole, by the hole (braces included). A `{what}` value is itself a wording,
    /// translated before it goes in.
    pub values: Vec<(&'static str, String)>,
    /// 0-based page.
    pub page: Option<usize>,
    /// The object the problem is in.
    pub object: Option<u32>,
    /// Where on the page, in user space.
    pub bbox: Option<[f64; 4]>,
    /// The verifier's rule id or clause, for hits from a standard.
    pub rule: Option<&'static str>,
    /// More about it: a wording for each name, and the value.
    pub details: Vec<(&'static str, String)>,
    /// A fixup that addresses it.
    pub fixup: Option<Fixup>,
}

impl Hit {
    /// A hit worded with one `{}` hole, or none.
    pub fn new(pattern: &'static str, value: Option<String>) -> Hit {
        let values = value.map(|v| vec![("{}", v)]).unwrap_or_default();
        Hit::worded(pattern, values)
    }

    /// A hit worded with named holes.
    pub fn worded(pattern: &'static str, values: Vec<(&'static str, String)>) -> Hit {
        let mut hit =
            Hit { message: String::new(), pattern, values, page: None, object: None, bbox: None, rule: None, details: Vec::new(), fixup: None };
        hit.message = hit.said(&crate::in_english);
        hit
    }

    pub fn on(mut self, page: Option<usize>) -> Hit {
        self.page = page;
        self
    }

    pub fn object(mut self, object: Option<u32>) -> Hit {
        self.object = object;
        self
    }

    pub fn bbox(mut self, bbox: Option<[f64; 4]>) -> Hit {
        self.bbox = bbox.filter(|b| b.iter().all(|v| v.is_finite()));
        self
    }

    pub fn detail(mut self, name: &'static str, value: String) -> Hit {
        self.details.push((name, value));
        self
    }

    pub fn fixup(mut self, fixup: Fixup) -> Hit {
        self.fixup = Some(fixup);
        self
    }

    /// The sentence said in the language `words` translates into. The wording is translated, a
    /// `{what}` wording in it is translated and goes in, and then the document's own words go in
    /// a single pass, so nothing the document says is read as a hole.
    pub fn said(&self, words: Words) -> String {
        let mut frame = words(self.pattern);
        if let Some((_, what)) = self.values.iter().find(|(hole, _)| *hole == "{what}") {
            frame = frame.replace("{what}", &words(what));
        }
        fill(&frame, &self.values)
    }
}

/// Fill a wording's holes in one pass. A hole is `{…}`; one not in `values` stays as written.
pub(crate) fn fill(pattern: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        // `{` and `}` are ASCII, so every index here is a char boundary.
        let Some(len) = rest.get(open..).and_then(|r| r.find('}')) else { break };
        let (before, hole, after) = (rest.get(..open), rest.get(open..open + len + 1), rest.get(open + len + 1..));
        let (Some(before), Some(hole), Some(after)) = (before, hole, after) else { break };
        out.push_str(before);
        match values.iter().find(|(name, _)| *name == hole) {
            Some((_, v)) => out.push_str(v),
            None => out.push_str(hole),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What became of a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// It found nothing.
    Passed,
    /// It found something.
    Found,
    /// It couldn't run here.
    Skipped,
}

impl Status {
    pub fn id(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Found => "found",
            Status::Skipped => "skipped",
        }
    }
}

/// One check's result.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub check: Check,
    pub severity: Severity,
    pub status: Status,
    pub hits: Vec<Hit>,
    /// Why it was skipped, as a wording.
    pub note: Option<&'static str>,
}

impl CheckResult {
    /// The fixups that address some of its hits, each once.
    pub fn fixups(&self) -> Vec<Fixup> {
        let mut seen = HashSet::new();
        self.hits.iter().filter_map(|h| h.fixup).filter(|f| seen.insert(*f)).collect()
    }
}

/// What [`run`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    pub profile: Profile,
    /// One per check of the profile, in its order.
    pub checks: Vec<CheckResult>,
    /// Pages whose content was too large or too deeply nested to read to the end: what is past
    /// that point wasn't checked.
    pub unread_pages: Vec<usize>,
    pub pages: usize,
}

impl Results {
    /// How many hits checks of `severity` found.
    pub fn count(&self, severity: Severity) -> usize {
        self.checks.iter().filter(|c| c.severity == severity).map(|c| c.hits.len()).sum()
    }

    /// Whether no check of severity Error found anything.
    pub fn passed(&self) -> bool {
        !self.checks.iter().any(|c| c.severity == Severity::Error && c.status == Status::Found)
    }

    /// The fixups that address what the checks found, each once, in check order.
    pub fn fixups(&self) -> Vec<Fixup> {
        let mut seen = HashSet::new();
        self.checks.iter().flat_map(CheckResult::fixups).filter(|f| seen.insert(*f)).collect()
    }
}

/// What a crate above this one runs for preflight.
pub trait Host {
    /// The Accessibility Checker's failed findings, or `None` when this host has no checker.
    fn accessibility(&self, doc: &Document) -> Option<Vec<Hit>>;
}

/// A host with nothing to add: the checks it would run are skipped.
pub struct NoHost;

impl Host for NoHost {
    fn accessibility(&self, _doc: &Document) -> Option<Vec<Hit>> {
        None
    }
}

/// Run `profile` over `doc`. Nothing is changed.
pub fn run(doc: &Document, profile: &Profile, host: &dyn Host) -> Results {
    let pages = pdfcraft_model::pages(doc);
    let specs = profile.checks.get(..MAX_CHECKS).unwrap_or(&profile.checks);
    // The hairline threshold the walk records strokes below: the widest any check asks for.
    let hairline_below = specs
        .iter()
        .filter_map(|c| match c.check {
            Check::Hairlines { min_width_pt } if min_width_pt.is_finite() => Some(min_width_pt.clamp(0.0, MAX_HAIRLINE_PT)),
            _ => None,
        })
        .fold(0.0, f64::max);
    let walk = specs.iter().any(|c| c.check.walks()).then(|| walk::Walk::run(doc, &pages, hairline_below));
    let empty = walk::Walk::default();
    let w = walk.as_ref().unwrap_or(&empty);
    let mut checks = Vec::with_capacity(specs.len());
    for s in specs {
        let (hits, note) = match &s.check {
            Check::FontsEmbedded => (checks::fonts(doc, w), None),
            Check::ColourSpaces { forbid } => (checks::colour_spaces(w, forbid), None),
            Check::SpotColours { max } => (checks::spot_colours(w, *max), None),
            Check::ImageResolution { colour_ppi, gray_ppi, bitmap_ppi } => (checks::image_resolution(w, *colour_ppi, *gray_ppi, *bitmap_ppi), None),
            Check::Hairlines { min_width_pt } => (checks::hairlines(w, *min_width_pt), None),
            Check::Transparency => (checks::transparency(w), None),
            Check::TrimBox => (checks::trim_box(&pages), None),
            Check::Bleed { min_mm } => (checks::bleed(doc, &pages, *min_mm), None),
            Check::Encryption => (checks::encryption(doc), None),
            Check::Scripts => (checks::scripts(doc, &pages), None),
            Check::AnnotationsInTrim => (checks::annotations_in_trim(doc, &pages), None),
            Check::Version { max } => (checks::version(doc, max.as_deref()), None),
            Check::Trapped => (checks::trapped(doc), None),
            Check::PdfA { level } => (checks::pdfa(doc, *level), None),
            Check::PdfX { level } => (checks::pdfx(doc, *level), None),
            Check::Accessibility => match host.accessibility(doc) {
                Some(hits) => (hits, None),
                None => (Vec::new(), Some("This check needs the Accessibility Checker, which isn't available here")),
            },
        };
        let status = if note.is_some() {
            Status::Skipped
        } else if hits.is_empty() {
            Status::Passed
        } else {
            Status::Found
        };
        checks.push(CheckResult { check: s.check.clone(), severity: s.severity, status, hits, note });
    }
    Results { profile: profile.clone(), checks, unread_pages: w.unread.clone(), pages: pages.len() }
}

/// Every wording a profile, a check or a hit says, in English, with holes where values go. An
/// interface that reads in another language translates these; `wordings_are_complete` fails if
/// something is said that isn't listed. Some are PDF/A's or PDF/X's wording for the same problem
/// and are translated already. What a standard's verifier reports is listed in its own
/// `WORDINGS`.
pub const WORDINGS: &[&str] = &[
    // Severities, statuses and the picker.
    "Errors",
    "Warnings",
    "Information",
    "Single checks",
    "This check needs the Accessibility Checker, which isn't available here",
    // Profiles.
    "Commercial print (CMYK)",
    "Checks a file for CMYK printing: fonts, colour, image resolution, hairlines, transparency, page boxes, encryption, JavaScript, annotations, Trapped and the PDF version",
    "Verifies the file against the standard, without changing it",
    "Accessibility",
    "Runs the Accessibility Checker's rules",
    // Checks.
    "Fonts not embedded",
    "Colour the profile doesn't allow",
    "Spot colours",
    "Image resolution",
    "Hairlines",
    "Transparency",
    "Pages without a TrimBox",
    "Bleed",
    "Encryption",
    "JavaScript and actions",
    "Annotations inside the trim",
    "PDF version",
    "Trapped key",
    "{} compliance",
    "Accessibility Checker rules",
    // Parameters and details.
    "Not allowed",
    "Most spot colours",
    "Colour images",
    "Gray images",
    "Bitmap images",
    "Thinnest line",
    "Least bleed",
    "Highest version",
    "Font type",
    "Pixels",
    "Placed size",
    "Least resolution",
    "Line width",
    "Flags",
    "Security handler",
    "Clause",
    "Rule",
    // Fonts.
    "The font {} isn't embedded",
    "A font isn't embedded",
    // Colour.
    "Content uses {} colour",
    "An image uses {} colour",
    "An inline image uses {} colour",
    "A shading uses {} colour",
    "Spot colour {}",
    "The pages use {} spot colours",
    // Images and lines.
    "A colour image is drawn at {} ppi",
    "A gray image is drawn at {} ppi",
    "A bitmap image is drawn at {} ppi",
    "The resolution of an image can't be measured",
    "A line is {} pt wide",
    "A line has width 0, the thinnest the printer can draw",
    "… and {} more on this page",
    // Transparency.
    "A graphics state uses a soft mask",
    "A graphics state sets an opacity below 1",
    "A graphics state uses the {} blend mode",
    "An image has a soft mask",
    "The page is a transparency group",
    "A form XObject is a transparency group",
    // Page boxes.
    "The page has neither a TrimBox nor an ArtBox",
    "The page has no BleedBox",
    "The bleed is {} mm on its narrowest side",
    // The document.
    "The document is encrypted",
    "The document has document-level JavaScript",
    "The document has additional actions (/AA)",
    "Page {} has additional actions",
    "A {} annotation has additional actions",
    "A form field has additional actions",
    "An object has additional actions (/AA)",
    "A {} action isn't allowed",
    "A {} annotation lies inside the trim",
    "The file is PDF {}",
    "The file is PDF {}, newer than the profile allows",
    "The document doesn't say whether it is trapped (Trapped)",
    "Trapped is {}, but it must be /True or /False",
    "The Trapped key and the metadata's pdf:Trapped disagree",
    // Fixups.
    "Set missing TrimBoxes from the CropBox",
    "Set Trapped to False",
    "Set Trapped to True",
    "Remove JavaScript and actions",
];

#[cfg(test)]
mod tests;
