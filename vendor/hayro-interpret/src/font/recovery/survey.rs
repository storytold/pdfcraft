//! PdfCraft patch (104): the document survey: how often each glyph of a font is shown across a
//! document, for rules whose evidence is the document's text as a whole rather than one font's
//! program or one page (`Rule::NEEDS_DOCUMENT_USAGE`). It runs only when such a rule can take a
//! kind of font it counts ([`SURVEYED_KINDS`]), and counts only fonts of those kinds.
//!
//! The survey reads the content of the first [`SURVEY_PAGES`] pages and of the form XObjects
//! they draw (nested up to [`MAX_FORM_DEPTH`] deep), follows `Tf` and counts every code each
//! text-showing operator (`Tj`, `TJ`, `'`, `"`) shows in a font that may need it: a Type 0 font
//! with an `Identity-H` or `Identity-V` encoding (two-byte codes that are CIDs, ISO 32000-2
//! §9.7.6.2), no `/ToUnicode` stream, no Adobe character collection, and an embedded TrueType,
//! OpenType or CID-keyed CFF program. Nothing is drawn and no glyph is loaded; annotations are
//! not read. A font is not built either: the survey reads of it only what it counts by
//! ([`SurveyFont`]), once per font object, the first time a page selects it. Each code is
//! counted as the number a rule reads for it ([`SurveyFont::number`], as
//! `Type0Font::recovery_number` gives it): the glyph id of a TrueType or OpenType program, the
//! CID of a CFF one.
//!
//! The survey is bounded by counts, never by time, so a document always gives the same evidence:
//! past [`MAX_SURVEY_OPERATORS`], [`MAX_SURVEY_GLYPHS`], [`MAX_SURVEY_FONTS`],
//! [`MAX_SURVEY_ENTRIES`], [`MAX_SURVEY_FORMS`] or [`MAX_SURVEY_BYTES`] it gives up, and the
//! document has no evidence ([`Evidence::OverCap`]). Fonts whose rules need the evidence then
//! read nothing in that document: no reading rather than a reading from part of it.

use super::document::{Evidence, FontUsage, UsageGroup};
use super::{Counter, FontKind, count};
use crate::InterpreterSettings;
use crate::font::cid::SurveyFont;
use hayro_cmap::CidFamily;
use hayro_syntax::Pdf;
use hayro_syntax::content::TypedIter;
use hayro_syntax::content::ops::TypedInstruction;
use hayro_syntax::object::dict::keys::{
    BASE_FONT, CID_FONT_TYPE0C, CIDSYSTEMINFO, CONTENTS, DESCENDANT_FONTS, ENCODING, FONT_DESC,
    FONT_FILE2, FONT_FILE3, FORM, OPEN_TYPE, ORDERING, REGISTRY, RESOURCES, SUBTYPE, TO_UNICODE,
    TYPE0,
};
use hayro_syntax::object::{Array, Dict, Name, Object, ObjectIdentifier, Stream};
use hayro_syntax::page::Resources;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The kinds of font the survey counts: Type 0 fonts with an `Identity-H` or `Identity-V`
/// encoding over an embedded TrueType, OpenType or CID-keyed CFF program, without a `/ToUnicode`
/// stream or an Adobe character collection. A rule that needs the usage of fonts of another
/// kind extends this (and [`candidate`] and [`SurveyFont`]), not the mechanism.
pub(crate) const SURVEYED_KINDS: [FontKind; 2] = [FontKind::CidTrueType, FontKind::CidCff];

/// Pages the survey reads, from the first: the rules that read its evidence were measured on
/// the first 400 pages of each document.
pub(crate) const SURVEY_PAGES: usize = 400;

/// Deepest form XObject nesting the survey follows (a deeper form is skipped).
const MAX_FORM_DEPTH: usize = 12;

/// Most content operators the survey reads across the document.
const MAX_SURVEY_OPERATORS: u64 = 1 << 26;

/// Most glyphs (codes shown) the survey counts across the document.
const MAX_SURVEY_GLYPHS: u64 = 1 << 26;

/// Most fonts the survey counts glyphs of.
const MAX_SURVEY_FONTS: usize = 4096;

/// Most different (font, code) pairs the survey keeps: what its counts hold in memory, and
/// what bounds the work of the rules that search them (each searches its font's numbers, and
/// its group's, once). The most any document of the Traditional Chinese corpora measured had
/// was 78,094.
const MAX_SURVEY_ENTRIES: usize = 1 << 19;

/// Most form XObjects the survey enters across the document (each time one is drawn).
const MAX_SURVEY_FORMS: u64 = 1 << 16;

/// Most decoded content bytes the survey reads across the document.
const MAX_SURVEY_BYTES: u64 = 1 << 30;

/// How often (in operators) the survey checks whether the read was cancelled.
const CANCEL_CHECK: u64 = 4096;

#[cfg(feature = "test-counters")]
thread_local! {
    /// A smaller [`MAX_SURVEY_GLYPHS`] for the surveys run on this thread (tests).
    pub(crate) static GLYPH_CAP: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// [`MAX_SURVEY_GLYPHS`], or a test's smaller cap.
fn glyph_cap() -> u64 {
    #[cfg(feature = "test-counters")]
    if let Some(cap) = GLYPH_CAP.with(std::cell::Cell::get) {
        return cap;
    }
    MAX_SURVEY_GLYPHS
}

/// How the program of a surveyed font is embedded, for grouping fonts (`/FontFile2`,
/// `/FontFile3` of subtype `OpenType`, or of subtype `CIDFontType0C`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ProgramFile {
    TrueType,
    OpenType,
    Cff,
}

/// The key that groups a document's fonts as subsets of one font: the Type 0 font's
/// `/BaseFont` without its subset tag (as written, byte for byte), the program's glyph count,
/// and how the program is embedded. Producers that embed a separate subset of the same font for
/// each part of a document give each the same key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PoolingKey {
    pub(crate) name: Vec<u8>,
    pub(crate) glyphs: Option<u32>,
    pub(crate) file: ProgramFile,
}

/// One font the survey counts.
struct Surveyed {
    /// Its key in the document (`document::font_key`).
    key: u128,
    /// Its dictionary's object, for reports.
    #[cfg_attr(
        not(feature = "test-counters"),
        allow(dead_code, reason = "read by test reports")
    )]
    id: Option<ObjectIdentifier>,
    font: SurveyFont,
    pooling: PoolingKey,
    /// Code shown → how often.
    counts: HashMap<u16, u64>,
}

/// Why the survey stopped early.
enum Stop {
    /// A count passed its cap: the document has no evidence.
    OverCap,
    /// The read was cancelled: nothing is kept, and a later read surveys again.
    Cancelled,
}

struct Survey<'s> {
    settings: &'s InterpreterSettings,
    /// The kinds of font counted: those of [`SURVEYED_KINDS`] a rule needs the usage of.
    kinds: &'s [FontKind],
    /// Font dictionary key → index into `fonts`, or `None` for a font not counted.
    classified: HashMap<u128, Option<usize>>,
    fonts: Vec<Surveyed>,
    operators: u64,
    glyphs: u64,
    entries: usize,
    forms: u64,
    bytes: u64,
}

/// Whether a font dictionary is a Type 0 font the survey may count (before its program is read):
/// an `Identity-H` or `Identity-V` encoding, no `/ToUnicode` stream, a descendant whose
/// `/CIDSystemInfo` is not an Adobe character collection (whose CIDs name their characters)
/// and whose descriptor embeds a TrueType, OpenType or CID-keyed CFF program, which also says
/// how it is embedded.
fn candidate(dict: &Dict<'_>) -> Option<ProgramFile> {
    if dict.get::<Name<'_>>(SUBTYPE).as_deref() != Some(TYPE0) {
        return None;
    }
    let encoding = dict.get::<Name<'_>>(ENCODING)?;
    if !matches!(encoding.as_ref(), b"Identity-H" | b"Identity-V") {
        return None;
    }
    if dict.get::<Stream<'_>>(TO_UNICODE).is_some() {
        return None;
    }
    let descendant = dict
        .get::<Array<'_>>(DESCENDANT_FONTS)?
        .iter::<Dict<'_>>()
        .next()?;
    if let Some(info) = descendant.get::<Dict<'_>>(CIDSYSTEMINFO)
        && let (Some(registry), Some(ordering)) = (
            info.get::<hayro_syntax::object::String<'_>>(REGISTRY),
            info.get::<hayro_syntax::object::String<'_>>(ORDERING),
        )
        && CidFamily::from_registry_ordering(registry.as_bytes(), ordering.as_bytes())
            .ucs2_cmap()
            .is_some()
    {
        return None;
    }
    let descriptor = descendant.get::<Dict<'_>>(FONT_DESC)?;
    if descriptor.get::<Stream<'_>>(FONT_FILE2).is_some() {
        return Some(ProgramFile::TrueType);
    }
    let file = descriptor.get::<Stream<'_>>(FONT_FILE3)?;
    match file.dict().get::<Name<'_>>(SUBTYPE)?.as_ref() {
        OPEN_TYPE => Some(ProgramFile::OpenType),
        CID_FONT_TYPE0C => Some(ProgramFile::Cff),
        _ => None,
    }
}

/// `/BaseFont` without its subset tag (six uppercase letters and `+`, ISO 32000-2 §9.9.2), as
/// written.
fn untagged_name(dict: &Dict<'_>) -> Vec<u8> {
    let name = dict
        .get::<Name<'_>>(BASE_FONT)
        .map(|n| n.as_ref().to_vec())
        .unwrap_or_default();
    match name.get(..7) {
        Some(tag)
            if tag.get(6) == Some(&b'+') && tag.iter().take(6).all(|b| b.is_ascii_uppercase()) =>
        {
            name.get(7..).map(<[u8]>::to_vec).unwrap_or_default()
        }
        _ => name,
    }
}

/// The decoded content of a page (its streams joined by a space), within what is left of
/// [`MAX_SURVEY_BYTES`].
fn page_content(page: &Dict<'_>, left: u64) -> Vec<u8> {
    let left = usize::try_from(left).unwrap_or(usize::MAX);
    let streams: Vec<Stream<'_>> = if let Some(stream) = page.get::<Stream<'_>>(CONTENTS) {
        vec![stream]
    } else if let Some(array) = page.get::<Array<'_>>(CONTENTS) {
        array.iter::<Stream<'_>>().collect()
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    for stream in streams {
        let room = left.saturating_sub(out.len());
        if room == 0 {
            break;
        }
        if let Ok(data) = stream.decoded_within(room) {
            out.extend_from_slice(&data);
            out.push(b' ');
        }
    }
    out
}

impl Survey<'_> {
    fn cancelled(&self) -> bool {
        self.settings
            .cancelled
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// The surveyed font a dictionary is, building it the first time.
    fn font(&mut self, dict: &Dict<'_>) -> Result<Option<usize>, Stop> {
        let key = super::document::font_key(dict);
        if let Some(known) = self.classified.get(&key) {
            return Ok(*known);
        }
        let surveyed = self.classify(dict, key)?;
        self.classified.insert(key, surveyed);
        Ok(surveyed)
    }

    fn classify(&mut self, dict: &Dict<'_>, key: u128) -> Result<Option<usize>, Stop> {
        let Some(file) = candidate(dict) else {
            return Ok(None);
        };
        let Some(font) = SurveyFont::read(dict) else {
            return Ok(None);
        };
        if !self.kinds.contains(&font.kind) {
            return Ok(None);
        }
        if self.fonts.len() >= MAX_SURVEY_FONTS {
            return Err(Stop::OverCap);
        }
        let glyphs = font.glyphs;
        self.fonts.push(Surveyed {
            key,
            id: dict.obj_id(),
            font,
            pooling: PoolingKey {
                name: untagged_name(dict),
                glyphs,
                file,
            },
            counts: HashMap::new(),
        });
        Ok(Some(self.fonts.len().saturating_sub(1)))
    }

    /// Count the two-byte codes of `shown` in surveyed font `font` (a last odd byte is no code).
    fn show(&mut self, font: Option<usize>, shown: &[u8]) -> Result<(), Stop> {
        let Some(surveyed) = font.and_then(|i| self.fonts.get_mut(i)) else {
            return Ok(());
        };
        for pair in shown.chunks_exact(2) {
            let (Some(&hi), Some(&lo)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            self.glyphs = self.glyphs.saturating_add(1);
            if self.glyphs > glyph_cap() {
                return Err(Stop::OverCap);
            }
            count(Counter::SurveyGlyph);
            let n = surveyed
                .counts
                .entry(u16::from_be_bytes([hi, lo]))
                .or_insert_with(|| {
                    self.entries = self.entries.saturating_add(1);
                    0
                });
            *n = n.saturating_add(1);
            if self.entries > MAX_SURVEY_ENTRIES {
                return Err(Stop::OverCap);
            }
        }
        Ok(())
    }

    /// Read one content stream with `resources`; `chain` holds the forms being drawn (a form
    /// that draws itself is not entered again).
    fn content(
        &mut self,
        data: &[u8],
        resources: &Resources<'_>,
        depth: usize,
        chain: &mut Vec<ObjectIdentifier>,
    ) -> Result<(), Stop> {
        let mut font: Option<usize> = None;
        let mut ops = TypedIter::new(data);
        while let Some(op) = ops.next() {
            self.operators = self.operators.saturating_add(1);
            if self.operators > MAX_SURVEY_OPERATORS {
                return Err(Stop::OverCap);
            }
            if self.operators % CANCEL_CHECK == 0 && self.cancelled() {
                return Err(Stop::Cancelled);
            }
            match op {
                TypedInstruction::TextFont(t) => {
                    font = match resources.get_font(&t.0) {
                        Some(dict) => self.font(&dict)?,
                        None => None,
                    };
                }
                TypedInstruction::ShowText(s) => self.show(font, s.0.as_bytes())?,
                TypedInstruction::NextLineAndShowText(s) => self.show(font, s.0.as_bytes())?,
                TypedInstruction::ShowTextWithParameters(s) => {
                    self.show(font, s.2.as_bytes())?;
                }
                TypedInstruction::ShowTexts(s) => {
                    for item in s.0.iter::<Object<'_>>() {
                        if let Object::String(text) = item {
                            self.show(font, text.as_bytes())?;
                        }
                    }
                }
                TypedInstruction::XObject(x) => {
                    if depth >= MAX_FORM_DEPTH {
                        continue;
                    }
                    let Some(form) = resources.get_x_object(&x.0) else {
                        continue;
                    };
                    let id = form.obj_id();
                    if form.dict().get::<Name<'_>>(SUBTYPE).as_deref() != Some(FORM)
                        || chain.contains(&id)
                    {
                        continue;
                    }
                    self.forms = self.forms.saturating_add(1);
                    if self.forms > MAX_SURVEY_FORMS {
                        return Err(Stop::OverCap);
                    }
                    let left = MAX_SURVEY_BYTES.saturating_sub(self.bytes);
                    let Ok(data) = form.decoded_within(usize::try_from(left).unwrap_or(usize::MAX))
                    else {
                        continue;
                    };
                    self.read_bytes(data.len())?;
                    let inner = Resources::for_form(
                        form.dict().get::<Dict<'_>>(RESOURCES),
                        resources.clone(),
                    );
                    chain.push(id);
                    let read = self.content(&data, &inner, depth.saturating_add(1), chain);
                    chain.pop();
                    read?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_bytes(&mut self, n: usize) -> Result<(), Stop> {
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.bytes > MAX_SURVEY_BYTES {
            return Err(Stop::OverCap);
        }
        Ok(())
    }

    fn pages(&mut self, pdf: &Pdf) -> Result<(), Stop> {
        for page in pdf.pages().iter().take(SURVEY_PAGES) {
            if self.cancelled() {
                return Err(Stop::Cancelled);
            }
            count(Counter::SurveyPage);
            let data = page_content(page.raw(), MAX_SURVEY_BYTES.saturating_sub(self.bytes));
            self.read_bytes(data.len())?;
            self.content(&data, page.resources(), 0, &mut Vec::new())?;
        }
        Ok(())
    }

    /// The evidence: each counted font's numbers, and the pooled numbers of each group of two
    /// or more fonts with the same [`PoolingKey`] (fonts that showed nothing are in no group).
    fn evidence(&self) -> Evidence {
        let mut numbered: Vec<(u128, &PoolingKey, Vec<(u32, u64)>, u64)> = Vec::new();
        for surveyed in &self.fonts {
            if surveyed.counts.is_empty() {
                continue;
            }
            let mut numbers: HashMap<u32, u64> = HashMap::new();
            for (code, n) in &surveyed.counts {
                if let Some(number) = surveyed.font.number(*code) {
                    let slot = numbers.entry(number).or_insert(0);
                    *slot = slot.saturating_add(*n);
                }
            }
            let shown = surveyed
                .counts
                .values()
                .fold(0_u64, |t, n| t.saturating_add(*n));
            numbered.push((surveyed.key, &surveyed.pooling, sorted(numbers), shown));
        }
        let mut groups: HashMap<&PoolingKey, (Vec<(Vec<(u32, u64)>, u64)>, HashMap<u32, u64>)> =
            HashMap::new();
        for (_, pooling, numbers, shown) in &numbered {
            let group = groups.entry(*pooling).or_default();
            group.0.push((numbers.clone(), *shown));
            for &(number, n) in numbers {
                let slot = group.1.entry(number).or_insert(0);
                *slot = slot.saturating_add(n);
            }
        }
        let groups: HashMap<&PoolingKey, Arc<UsageGroup>> = groups
            .into_iter()
            .filter(|(_, (members, _))| members.len() >= 2)
            .map(|(key, (members, numbers))| {
                let group = UsageGroup::new(key.name.clone(), members, sorted(numbers));
                (key, Arc::new(group))
            })
            .collect();
        let fonts = numbered
            .iter()
            .map(|(key, pooling, numbers, shown)| {
                let usage = FontUsage {
                    numbers: numbers.clone(),
                    shown: *shown,
                    group: groups.get(pooling).cloned(),
                };
                (*key, Arc::new(usage))
            })
            .collect();
        Evidence::Counted(fonts)
    }
}

/// `numbers` as (number, count) pairs in number order.
fn sorted(numbers: HashMap<u32, u64>) -> Vec<(u32, u64)> {
    let mut v: Vec<(u32, u64)> = numbers.into_iter().collect();
    v.sort_unstable();
    v
}

/// Whether a resource dictionary chain, or the forms it draws, names a font the survey may
/// count: the cheap check that spares a document without one the reading of its content.
fn names_candidate(
    resources: &Resources<'_>,
    depth: usize,
    seen: &mut HashSet<ObjectIdentifier>,
) -> bool {
    let mut level = Some(resources);
    while let Some(r) = level {
        for name in r.fonts.keys() {
            if r.fonts
                .get::<Dict<'_>>(name.as_ref())
                .is_some_and(|d| candidate(&d).is_some())
            {
                return true;
            }
        }
        if depth < MAX_FORM_DEPTH {
            for name in r.x_objects.keys() {
                let Some(form) = r.x_objects.get::<Stream<'_>>(name.as_ref()) else {
                    continue;
                };
                // `seen` caps the forms looked at across the document as well.
                if seen.len() as u64 >= MAX_SURVEY_FORMS || !seen.insert(form.obj_id()) {
                    continue;
                }
                if form.dict().get::<Name<'_>>(SUBTYPE).as_deref() != Some(FORM) {
                    continue;
                }
                let inner = Resources::for_form(form.dict().get::<Dict<'_>>(RESOURCES), r.clone());
                if names_candidate(&inner, depth.saturating_add(1), seen) {
                    return true;
                }
            }
        }
        level = r.parent();
    }
    false
}

/// Survey `pdf` for fonts of `kinds` (see the module documentation). `None` when the read was
/// cancelled (`InterpreterSettings::cancelled`): nothing is kept, and a later read surveys
/// again.
pub(crate) fn survey(
    pdf: &Pdf,
    settings: &InterpreterSettings,
    kinds: &[FontKind],
) -> Option<Evidence> {
    count(Counter::SurveyRun);
    let mut seen = HashSet::new();
    let any = pdf
        .pages()
        .iter()
        .take(SURVEY_PAGES)
        .any(|page| names_candidate(page.resources(), 0, &mut seen));
    if !any {
        return Some(Evidence::Counted(HashMap::new()));
    }
    let mut survey = Survey {
        settings,
        kinds,
        classified: HashMap::new(),
        fonts: Vec::new(),
        operators: 0,
        glyphs: 0,
        entries: 0,
        forms: 0,
        bytes: 0,
    };
    match survey.pages(pdf) {
        Ok(()) => Some(survey.evidence()),
        Err(Stop::OverCap) => Some(Evidence::OverCap),
        Err(Stop::Cancelled) => None,
    }
}

/// One font of a [`report`].
#[cfg(feature = "test-counters")]
pub(crate) struct Reported {
    /// Its dictionary's object number.
    pub(crate) object: Option<i32>,
    /// `/BaseFont` as written.
    pub(crate) base: Vec<u8>,
    /// (code, times shown, the number a rule reads for it).
    pub(crate) codes: Vec<(u16, u64, Option<u32>)>,
    /// The font's evidence, as a rule's guard gets it.
    #[allow(dead_code, reason = "read by the rules' own reports")]
    pub(crate) usage: Option<Arc<FontUsage>>,
}

/// Test and verification hook: the survey of `pdf`, font by font (`None` when cancelled; the
/// flag says whether it passed a cap).
#[cfg(feature = "test-counters")]
pub(crate) fn report(pdf: &Pdf, settings: &InterpreterSettings) -> Option<(bool, Vec<Reported>)> {
    let kinds = SURVEYED_KINDS;
    let kinds = kinds.as_slice();
    let mut survey = Survey {
        settings,
        kinds,
        classified: HashMap::new(),
        fonts: Vec::new(),
        operators: 0,
        glyphs: 0,
        entries: 0,
        forms: 0,
        bytes: 0,
    };
    let evidence = match survey.pages(pdf) {
        Ok(()) => survey.evidence(),
        Err(Stop::OverCap) => Evidence::OverCap,
        Err(Stop::Cancelled) => return None,
    };
    let over = matches!(evidence, Evidence::OverCap);
    let fonts = survey
        .fonts
        .iter()
        .map(|f| {
            let mut codes: Vec<(u16, u64, Option<u32>)> = f
                .counts
                .iter()
                .map(|(&c, &n)| (c, n, f.font.number(c)))
                .collect();
            codes.sort_unstable();
            let usage = match &evidence {
                Evidence::Counted(map) => map.get(&f.key).cloned(),
                Evidence::OverCap => None,
            };
            Reported {
                object: f.id.map(|id| id.obj_number),
                base: f.pooling.name.clone(),
                codes,
                usage,
            }
        })
        .collect();
    Some((over, fonts))
}

/// How the survey classifies one font: the kind, the program's glyph count and the number read
/// for every code (`None`: not counted).
#[cfg(feature = "test-counters")]
pub(crate) type Classified = Option<(FontKind, Option<u32>, Vec<Option<u32>>)>;

/// Test hook: every Type 0 font the first [`SURVEY_PAGES`] pages name in their own resources
/// (each object once), classified as the survey does ([`SurveyFont`]) and by building the font
/// ([`crate::font::cid::Type0Font`]) and asking it the same, as the survey did before it read
/// fonts without building them: (object number, the survey's, the built font's).
#[cfg(feature = "test-counters")]
pub(crate) fn classifications(
    pdf: &Pdf,
    settings: &InterpreterSettings,
) -> Vec<(Option<i32>, Classified, Classified)> {
    use crate::font::cid::Type0Font;
    let codes = || 0..=u16::MAX;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for page in pdf.pages().iter().take(SURVEY_PAGES) {
        let fonts = &page.resources().fonts;
        for name in fonts.keys() {
            let Some(dict) = fonts.get::<Dict<'_>>(name.as_ref()) else {
                continue;
            };
            if dict.get::<Name<'_>>(SUBTYPE).as_deref() != Some(TYPE0)
                || !seen.insert(super::document::font_key(&dict))
            {
                continue;
            }
            let light = candidate(&dict).and_then(|_| SurveyFont::read(&dict)).map(|f| {
                let numbers = codes().map(|c| f.number(c)).collect();
                (f.kind, f.glyphs, numbers)
            });
            let built = candidate(&dict)
                .and_then(|_| Type0Font::new(&dict, &settings.font_resolver, &settings.cmap_resolver))
                .filter(|f| {
                    f.identity_encoding()
                        && !f.adobe_collection()
                        && SURVEYED_KINDS.contains(&f.recovery_font_kind())
                })
                .map(|f| {
                    let numbers = codes()
                        .map(|c| f.recovery_number(u32::from(c)).map(|(_, n)| n))
                        .collect();
                    (f.recovery_font_kind(), f.program_glyphs(), numbers)
                });
            out.push((dict.obj_id().map(|id| id.obj_number), light, built));
        }
    }
    out
}
