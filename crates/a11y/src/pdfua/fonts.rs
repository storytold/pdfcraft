//! The font conditions: Matterhorn checkpoint 31 (ISO 14289-1 §7.21), what each font the page
//! content uses must be, and 17-003, that the characters of a formula map to Unicode. They read the
//! font's dictionary, its CMap and ToUnicode map, its embedded program (`pdfcraft_fonts::program`)
//! and the codes the content shows with it.
//!
//! Where a font holds what PdfCraft can't read, the condition is left undecided for the file and
//! reported as not checked, never passed: a program that doesn't parse, a predefined CMap other
//! than Identity (PdfCraft reads their names, from ISO 32000-1 Table 118, not their mappings), or a
//! glyph name outside the Annex D Latin set and the names the Adobe Glyph List's rules spell
//! (`uniXXXX`, `uXXXX`, ligatures and suffixes of those): PdfCraft doesn't hold the list itself.
//!
//! Every sentence a finding here can say is listed in [`crate::WORDINGS`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::Std14;
use pdfcraft_fonts::pdf::{BaseEncoding, MAX_CMAP_BYTES, Metrics, cmap_program, glyph_text, glyph_unicode, latin_glyph};
use pdfcraft_fonts::program::{Face, Program, Unreadable};

/// A question about a font PdfCraft can't answer for this file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Undecided;

use super::Collected;
use crate::Finding;
use crate::content::Shown;

/// Distinct codes kept for one font, and for all fonts together (guards against absurd files).
const MAX_CODES: usize = 1 << 16;
const MAX_ALL_CODES: usize = 1 << 21;

/// CIDs a CIDSet or a CIDToGIDMap stream is read for, at most.
const MAX_CIDS: usize = 1 << 16;

/// Bytes an embedded CMap, a ToUnicode map or a Type 3 glyph description may decode to.
const MAX_STREAM: usize = 1 << 22;

/// Widths in text space that differ by this much or less are the same: Matterhorn 31-016 fails
/// a difference of more than 1/1000 of a unit, the dictionary's own precision.
const WIDTH_TOLERANCE: f64 = 0.001 + 1e-9;

/// The predefined CMaps of ISO 32000-1 Table 118, with the Adobe character collection
/// (`Ordering`) whose CIDs their codes select. Identity-H and Identity-V select none in particular.
const PREDEFINED: &[(&[u8], &[u8])] = &[
    (b"GB-EUC-H", b"GB1"),
    (b"GB-EUC-V", b"GB1"),
    (b"GBpc-EUC-H", b"GB1"),
    (b"GBpc-EUC-V", b"GB1"),
    (b"GBK-EUC-H", b"GB1"),
    (b"GBK-EUC-V", b"GB1"),
    (b"GBKp-EUC-H", b"GB1"),
    (b"GBKp-EUC-V", b"GB1"),
    (b"GBK2K-H", b"GB1"),
    (b"GBK2K-V", b"GB1"),
    (b"UniGB-UCS2-H", b"GB1"),
    (b"UniGB-UCS2-V", b"GB1"),
    (b"UniGB-UTF16-H", b"GB1"),
    (b"UniGB-UTF16-V", b"GB1"),
    (b"B5pc-H", b"CNS1"),
    (b"B5pc-V", b"CNS1"),
    (b"HKscs-B5-H", b"CNS1"),
    (b"HKscs-B5-V", b"CNS1"),
    (b"ETen-B5-H", b"CNS1"),
    (b"ETen-B5-V", b"CNS1"),
    (b"ETenms-B5-H", b"CNS1"),
    (b"ETenms-B5-V", b"CNS1"),
    (b"CNS-EUC-H", b"CNS1"),
    (b"CNS-EUC-V", b"CNS1"),
    (b"UniCNS-UCS2-H", b"CNS1"),
    (b"UniCNS-UCS2-V", b"CNS1"),
    (b"UniCNS-UTF16-H", b"CNS1"),
    (b"UniCNS-UTF16-V", b"CNS1"),
    (b"83pv-RKSJ-H", b"Japan1"),
    (b"90ms-RKSJ-H", b"Japan1"),
    (b"90ms-RKSJ-V", b"Japan1"),
    (b"90msp-RKSJ-H", b"Japan1"),
    (b"90msp-RKSJ-V", b"Japan1"),
    (b"90pv-RKSJ-H", b"Japan1"),
    (b"Add-RKSJ-H", b"Japan1"),
    (b"Add-RKSJ-V", b"Japan1"),
    (b"EUC-H", b"Japan1"),
    (b"EUC-V", b"Japan1"),
    (b"Ext-RKSJ-H", b"Japan1"),
    (b"Ext-RKSJ-V", b"Japan1"),
    (b"H", b"Japan1"),
    (b"V", b"Japan1"),
    (b"UniJIS-UCS2-H", b"Japan1"),
    (b"UniJIS-UCS2-V", b"Japan1"),
    (b"UniJIS-UCS2-HW-H", b"Japan1"),
    (b"UniJIS-UCS2-HW-V", b"Japan1"),
    (b"UniJIS-UTF16-H", b"Japan1"),
    (b"UniJIS-UTF16-V", b"Japan1"),
    (b"KSC-EUC-H", b"Korea1"),
    (b"KSC-EUC-V", b"Korea1"),
    (b"KSCms-UHC-H", b"Korea1"),
    (b"KSCms-UHC-V", b"Korea1"),
    (b"KSCms-UHC-HW-H", b"Korea1"),
    (b"KSCms-UHC-HW-V", b"Korea1"),
    (b"KSCpc-EUC-H", b"Korea1"),
    (b"UniKS-UCS2-H", b"Korea1"),
    (b"UniKS-UCS2-V", b"Korea1"),
    (b"UniKS-UTF16-H", b"Korea1"),
    (b"UniKS-UTF16-V", b"Korea1"),
    (b"Identity-H", b"Identity"),
    (b"Identity-V", b"Identity"),
];

/// The character collections of 31-027's third exception: a Type 0 font over one of them can go
/// without a ToUnicode map, its CIDs having published Unicode values.
const KNOWN_COLLECTIONS: &[&[u8]] = &[b"GB1", b"CNS1", b"Japan1", b"Korea1"];

fn identity(name: &[u8]) -> bool {
    matches!(name, b"Identity-H" | b"Identity-V")
}

fn predefined(name: &[u8]) -> Option<&'static [u8]> {
    PREDEFINED.iter().find(|(n, _)| *n == name).map(|(_, ordering)| *ordering)
}

fn pdf_name(n: &[u8]) -> String {
    String::from_utf8_lossy(n).into_owned()
}

/// How the content uses one code of a font.
#[derive(Clone, Copy, Debug)]
pub(super) struct Use {
    /// The first page that shows it.
    page: usize,
    /// Some showing of it draws it: not all of them are in text render mode 3 (invisible).
    rendered: bool,
}

/// The codes each font shows, collected during the content scan.
pub(super) struct Codes<'a> {
    doc: &'a Document,
    /// Font → its metrics, read the first time it shows something (`None`: not a dictionary).
    metrics: HashMap<ObjRef, Option<Metrics>>,
    used: HashMap<ObjRef, BTreeMap<u32, Use>>,
    kept: usize,
    /// Distinct codes kept for one font, and for all fonts together.
    limits: (usize, usize),
    /// Some code wasn't kept: the limits were reached, so what the codes decide stays undecided.
    truncated: bool,
    /// (page, MCID) of the content of each Formula element.
    formulas: HashSet<(usize, i64)>,
    /// Font → each code it shows in a formula, with the first page that does.
    in_formulas: HashMap<ObjRef, BTreeMap<u32, usize>>,
}

impl<'a> Codes<'a> {
    pub(super) fn new(doc: &'a Document, formulas: HashSet<(usize, i64)>) -> Self {
        Codes::with_limits(doc, formulas, (MAX_CODES, MAX_ALL_CODES))
    }

    /// [`Codes::new`] keeping at most `limits.0` distinct codes a font and `limits.1` in all.
    pub(super) fn with_limits(doc: &'a Document, formulas: HashSet<(usize, i64)>, limits: (usize, usize)) -> Self {
        Codes { doc, metrics: HashMap::new(), used: HashMap::new(), kept: 0, limits, truncated: false, formulas, in_formulas: HashMap::new() }
    }
}

impl Shown for Codes<'_> {
    fn show(&mut self, page: usize, font: ObjRef, bytes: &[u8], rendered: bool, mcid: Option<i64>) {
        let doc = self.doc;
        let Some(metrics) = self.metrics.entry(font).or_insert_with(|| doc.get(font).as_dict().map(|d| Metrics::from_dict(doc, d))) else { return };
        let formula = mcid.is_some_and(|m| self.formulas.contains(&(page, m)));
        let codes = self.used.entry(font).or_default();
        for (code, _) in metrics.codes(bytes) {
            let full = codes.len() >= self.limits.0 || self.kept >= self.limits.1;
            match codes.get_mut(&code) {
                Some(u) => u.rendered |= rendered,
                None if full => self.truncated = true,
                None => {
                    codes.insert(code, Use { page, rendered });
                    self.kept += 1;
                }
            }
            if formula {
                let in_formula = self.in_formulas.entry(font).or_default();
                if in_formula.len() < self.limits.0 {
                    in_formula.entry(code).or_insert(page);
                } else if !in_formula.contains_key(&code) {
                    self.truncated = true;
                }
            }
        }
    }
}

/// Check every font `fonts` lists (page → fonts the content selects) against the font conditions,
/// with the codes `codes` saw them show.
pub(super) fn check(doc: &Document, fonts: &BTreeMap<usize, BTreeSet<ObjRef>>, codes: &Codes, c: &mut Collected) {
    // Each font once, at the first page that uses it; pages are in order.
    let mut first: HashMap<ObjRef, usize> = HashMap::new();
    for (p, refs) in fonts {
        for r in refs {
            first.entry(*r).or_insert(*p);
        }
    }
    let mut order: Vec<(usize, ObjRef)> = first.into_iter().map(|(r, p)| (p, r)).collect();
    order.sort_unstable();
    for (page, r) in order {
        let Some(dict) = doc.get(r).as_dict().cloned() else { continue };
        let read;
        let metrics = match codes.metrics.get(&r).and_then(Option::as_ref) {
            Some(m) => m,
            None => {
                read = Metrics::from_dict(doc, &dict);
                &read
            }
        };
        let font = Font::new(doc, r, &dict, page, metrics, codes.used.get(&r));
        font.check(codes.in_formulas.get(&r), c);
    }
    if codes.truncated {
        for id in ["31-011", "31-016", "31-018", "31-027", "31-030", "17-003"] {
            c.undecided(id);
        }
    }
}

/// The kind of font a dictionary describes (ISO 32000-1 §9.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Type0,
    /// Type 1, and multiple master Type 1.
    Type1,
    TrueType,
    Type3,
    Other,
}

/// What a simple font's encoding calls one code.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Named {
    /// The glyph of this name.
    Is(String),
    /// Nothing: the encoding leaves the code undefined.
    Undefined,
    /// Whatever the embedded program's built-in encoding calls it.
    Builtin,
    /// An encoding PdfCraft holds no table for: MacExpert, or a symbolic font's own one, not embedded.
    Unknown,
}

/// What a code selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    /// A glyph the program has.
    Found(u32),
    /// A glyph the program doesn't have.
    Missing,
    /// The .notdef glyph.
    Notdef,
    /// A glyph other than .notdef, in a font with no program to look in.
    Named,
    /// PdfCraft can't tell.
    Unknown,
}

/// A CIDSystemInfo dictionary: the character collection a CMap or CIDFont is of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SystemInfo {
    registry: Option<Vec<u8>>,
    ordering: Option<Vec<u8>>,
    /// Unknown for a predefined CMap, whose Supplement is in the CMap itself.
    supplement: Option<i64>,
}

/// A TrueType CIDFont's CID → glyph id map.
#[derive(Clone, Debug, PartialEq, Eq)]
enum GidMap {
    Identity,
    Table(Vec<u16>),
    /// Neither: something PdfCraft can't read as a map.
    Unknown,
}

/// One font, and what the checks need to know of it.
struct Font<'a> {
    doc: &'a Document,
    r: ObjRef,
    dict: &'a Dict,
    kind: Kind,
    /// The first page that uses it.
    page: usize,
    metrics: &'a Metrics,
    used: Option<&'a BTreeMap<u32, Use>>,
    /// A Type 0 font's CIDFont.
    cidfont: Option<Dict>,
    /// The font's descriptor: a Type 0 font's is its CIDFont's.
    descriptor: Option<Dict>,
    /// The descriptor's Symbolic flag (bit 3).
    symbolic: bool,
    /// Simple fonts: what each code from 0 to 255 is called.
    names: Vec<Named>,
    /// Type 0 fonts: PdfCraft reads which CID each code selects (Identity, or an embedded CMap
    /// that uses no other).
    cids_known: bool,
    gid_map: GidMap,
}

impl<'a> Font<'a> {
    fn new(doc: &'a Document, r: ObjRef, dict: &'a Dict, page: usize, metrics: &'a Metrics, used: Option<&'a BTreeMap<u32, Use>>) -> Self {
        let kind = match dict.name(b"Subtype") {
            Some(b"Type0") => Kind::Type0,
            Some(b"Type1" | b"MMType1") => Kind::Type1,
            Some(b"TrueType") => Kind::TrueType,
            Some(b"Type3") => Kind::Type3,
            _ => Kind::Other,
        };
        let dict_at = |d: &Dict, key: &[u8]| d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned());
        let cidfont = (kind == Kind::Type0)
            .then(|| dict.get(b"DescendantFonts").map(|d| doc.resolve(d)).and_then(|a| a.as_array().and_then(|a| a.first().cloned())))
            .flatten()
            .and_then(|d| doc.resolve(&d).as_dict().cloned());
        let descriptor = dict_at(cidfont.as_ref().unwrap_or(dict), b"FontDescriptor");
        let flags = descriptor.as_ref().and_then(|d| d.get(b"Flags")).and_then(|f| doc.resolve(f).as_f64()).unwrap_or(0.0);
        // Flags is an integer; a real one is read for its integer part, and nonsense as no flags.
        let symbolic = flags.is_finite() && flags.abs() < 4.0e9 && (flags as i64) & 4 != 0;
        let mut font = Font {
            doc,
            r,
            dict,
            kind,
            page,
            metrics,
            used,
            cidfont,
            descriptor,
            symbolic,
            names: Vec::new(),
            cids_known: false,
            gid_map: GidMap::Unknown,
        };
        let embedded = font.descriptor.as_ref().is_some_and(|d| d.contains(b"FontFile") || d.contains(b"FontFile2") || d.contains(b"FontFile3"));
        font.names = font.encoding_names(embedded);
        font.cids_known = font.reads_cids();
        font.gid_map = font.read_gid_map();
        font
    }

    fn resolve(&self, d: &Dict, key: &[u8]) -> Option<std::sync::Arc<Object>> {
        d.get(key).map(|o| self.doc.resolve(o))
    }

    /// The font's name as findings give it: its BaseFont, else its object number.
    fn label(&self) -> String {
        self.dict.name(b"BaseFont").map(pdf_name).unwrap_or_else(|| self.r.to_string())
    }

    fn finding_at(&self, page: usize, pattern: &'static str, mut values: Vec<(&'static str, String)>) -> Finding {
        values.insert(0, ("{}", self.label()));
        values.push(("{p}", (page + 1).to_string()));
        let mut f = Finding::worded(Some(page), pattern, values);
        f.object = Some(self.r);
        f
    }

    fn finding(&self, pattern: &'static str, values: Vec<(&'static str, String)>) -> Finding {
        self.finding_at(self.page, pattern, values)
    }

    /// A code as a PDF hex string: one byte for a simple font, two or more for a composite one.
    fn code_text(&self, code: u32) -> String {
        let bytes = match (self.kind, code) {
            (Kind::Type0, 0..=0xFFFF) => 2,
            (Kind::Type0, 0x1_0000..=0xFF_FFFF) => 3,
            (Kind::Type0, _) => 4,
            (_, 0..=0xFF) => 1,
            _ => 4,
        };
        format!("<{code:0width$X}>", width = bytes * 2)
    }

    /// What each code of a simple font is called (ISO 32000-1 §9.6.6): its `/Differences` name,
    /// else its base encoding's, else the program's built-in encoding's. A non-symbolic TrueType
    /// font fills what is left undefined from StandardEncoding (§9.6.6.4).
    fn encoding_names(&self, embedded: bool) -> Vec<Named> {
        if !matches!(self.kind, Kind::Type1 | Kind::TrueType | Kind::Type3) {
            return Vec::new();
        }
        let table =
            |t: BaseEncoding| (0..=255u8).map(|code| t.glyph_name(code).map_or(Named::Undefined, |n| Named::Is(n.to_owned()))).collect::<Vec<_>>();
        let named = |n: &[u8]| match BaseEncoding::from_name(n) {
            Some(t) => table(t),
            None => vec![Named::Unknown; 256],
        };
        // Symbol and ZapfDingbats have built-in encodings of their own, which PdfCraft holds no
        // names for, whatever their descriptor's flags say.
        let own_builtin =
            matches!(self.dict.name(b"BaseFont").and_then(|b| Std14::from_base_font(&pdf_name(b))), Some(Std14::Symbol | Std14::ZapfDingbats));
        let implicit = || match self.kind {
            // A Type 3 font has no built-in encoding: what Differences leaves out is undefined.
            Kind::Type3 => vec![Named::Undefined; 256],
            Kind::TrueType if !self.symbolic => table(BaseEncoding::Standard),
            _ if embedded => vec![Named::Builtin; 256],
            _ if !self.symbolic && !own_builtin => table(BaseEncoding::Standard),
            _ => vec![Named::Unknown; 256],
        };
        let encoding = self.resolve(self.dict, b"Encoding");
        let (mut names, differences) = match encoding.as_deref() {
            None => (implicit(), None),
            Some(Object::Name(n)) => (named(n), None),
            Some(Object::Dict(d)) => {
                let base = d.name(b"BaseEncoding").map_or_else(implicit, named);
                (base, d.get(b"Differences").map(|x| self.doc.resolve(x)).and_then(|x| x.as_array().cloned()))
            }
            Some(_) => (vec![Named::Unknown; 256], None),
        };
        let mut code: Option<usize> = None;
        for item in differences.unwrap_or_default() {
            match self.doc.resolve(&item).as_ref() {
                Object::Int(n) => code = usize::try_from(*n).ok(),
                Object::Name(n) => {
                    if let Some(slot) = code.and_then(|c| names.get_mut(c)) {
                        *slot = Named::Is(pdf_name(n));
                    }
                    code = code.and_then(|c| c.checked_add(1));
                }
                _ => {}
            }
        }
        if self.kind == Kind::TrueType && !self.symbolic {
            for (code, slot) in names.iter_mut().enumerate() {
                if *slot == Named::Undefined
                    && let Some(n) = u8::try_from(code).ok().and_then(|c| BaseEncoding::Standard.glyph_name(c))
                {
                    *slot = Named::Is(n.to_owned());
                }
            }
        }
        names
    }

    fn named(&self, code: u32) -> &Named {
        usize::try_from(code).ok().and_then(|c| self.names.get(c)).unwrap_or(&Named::Unknown)
    }

    /// A Type 0 font's CMap is Identity, or embedded and built on no other: PdfCraft reads the
    /// CID of each code.
    fn reads_cids(&self) -> bool {
        match self.resolve(self.dict, b"Encoding").as_deref() {
            Some(Object::Name(n)) => identity(n),
            Some(Object::Stream(s)) => {
                !s.dict.contains(b"UseCMap") && s.decoded_within(MAX_STREAM).is_ok_and(|data| cmap_program(&data).uses.is_empty())
            }
            _ => false,
        }
    }

    fn read_gid_map(&self) -> GidMap {
        let Some(cidfont) = &self.cidfont else { return GidMap::Unknown };
        match self.resolve(cidfont, b"CIDToGIDMap").as_deref() {
            // Identity is the default (ISO 32000-1 Table 117), though PDF/UA wants it said (31-005).
            None => GidMap::Identity,
            Some(Object::Name(n)) if n.as_slice() == b"Identity" => GidMap::Identity,
            Some(Object::Stream(s)) => match s.decoded_within(MAX_CIDS * 2) {
                Ok(data) => GidMap::Table(data.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect()),
                Err(_) => GidMap::Unknown,
            },
            Some(_) => GidMap::Unknown,
        }
    }

    fn check(&self, formulas: Option<&BTreeMap<u32, usize>>, c: &mut Collected) {
        let program = match &self.descriptor {
            Some(d) if self.kind != Kind::Type3 => Program::embedded(self.doc, d),
            _ => Ok(None),
        };
        let face = program.as_ref().ok().and_then(Option::as_ref).and_then(Program::face);
        match self.kind {
            Kind::Type0 => self.composite(face.as_ref(), &program, c),
            Kind::TrueType => self.truetype(face.as_ref(), &program, c),
            Kind::Type1 => self.charset(face.as_ref(), &program, c),
            Kind::Type3 | Kind::Other => {}
        }
        self.glyphs(face.as_ref(), &program, c);
        self.to_unicode(face.as_ref(), c);
        if let Some(codes) = formulas {
            self.formula(codes, face.as_ref(), c);
        }
    }

    /// 31-001 to 31-008, 31-014 and 31-015: a Type 0 font's CMap and CIDFont.
    fn composite(&self, face: Option<&Face>, program: &Result<Option<Program>, Unreadable>, c: &mut Collected) {
        // The CMap's character collection, and its Supplement when PdfCraft can read it.
        let mut cmap: Option<SystemInfo> = None;
        match self.resolve(self.dict, b"Encoding").as_deref() {
            Some(Object::Name(n)) if identity(n) => {}
            Some(Object::Name(n)) => match predefined(n) {
                Some(ordering) => cmap = Some(SystemInfo { registry: Some(b"Adobe".to_vec()), ordering: Some(ordering.to_vec()), supplement: None }),
                None => c.add(
                    "31-006",
                    self.finding("Font {} on page {p}: its CMap {cmap} is neither predefined nor embedded", vec![("{cmap}", pdf_name(n))]),
                ),
            },
            Some(Object::Stream(s)) => {
                cmap = self.system_info(&s.dict);
                if cmap.is_none() {
                    for id in ["31-001", "31-002", "31-003"] {
                        c.undecided(id);
                    }
                }
                match s.decoded_within(MAX_STREAM) {
                    Ok(data) => {
                        let read = cmap_program(&data);
                        let wmode = s.dict.get(b"WMode").and_then(|w| self.doc.resolve(w).as_int()).unwrap_or(0);
                        if read.wmode.unwrap_or(0) != wmode {
                            c.add(
                                "31-007",
                                self.finding("Font {} on page {p}: its CMap's WMode differs from the one in the CMap stream", Vec::new()),
                            );
                        }
                        for used in read.uses.iter().filter(|u| predefined(u).is_none()) {
                            c.add(
                                "31-008",
                                self.finding(
                                    "Font {} on page {p}: its CMap uses CMap {cmap}, which isn't predefined",
                                    vec![("{cmap}", pdf_name(used))],
                                ),
                            );
                        }
                    }
                    Err(_) => {
                        c.undecided("31-007");
                        c.undecided("31-008");
                    }
                }
                match self.resolve(&s.dict, b"UseCMap").as_deref() {
                    Some(Object::Name(n)) if predefined(n).is_none() => {
                        c.add(
                            "31-008",
                            self.finding("Font {} on page {p}: its CMap uses CMap {cmap}, which isn't predefined", vec![("{cmap}", pdf_name(n))]),
                        );
                    }
                    Some(Object::Stream(_)) => {
                        c.add("31-008", self.finding("Font {} on page {p}: its CMap uses an embedded CMap, not a predefined one", Vec::new()));
                    }
                    _ => {}
                }
            }
            _ => c.add("31-006", self.finding("Font {} on page {p} has no CMap (Encoding)", Vec::new())),
        }
        let cidfont = self.cidfont.clone().unwrap_or_default();
        // A CMap other than Identity selects CIDs of one character collection, so the CIDFont must
        // be of that collection, at a Supplement no higher than the CMap's (ISO 14289-1 7.21.3.1).
        // A CIDFont without one counts as Supplement 0.
        if let Some(cmap) = cmap {
            let font = self.system_info(&cidfont).unwrap_or_default();
            if cmap.registry != font.registry {
                c.add(
                    "31-001",
                    self.finding("Font {} on page {p}: its CMap and CIDFont name different registries (CIDSystemInfo Registry)", Vec::new()),
                );
            }
            if cmap.ordering != font.ordering {
                c.add(
                    "31-002",
                    self.finding("Font {} on page {p}: its CMap and CIDFont name different orderings (CIDSystemInfo Ordering)", Vec::new()),
                );
            }
            match cmap.supplement {
                Some(s) if font.supplement.unwrap_or(0) > s => {
                    c.add("31-003", self.finding("Font {} on page {p}: its CIDFont's Supplement is higher than its CMap's", Vec::new()));
                }
                Some(_) => {}
                // A predefined CMap's Supplement is in the CMap itself, which PdfCraft doesn't read.
                None => c.undecided("31-003"),
            }
        }
        if cidfont.name(b"Subtype") == Some(b"CIDFontType2") {
            match self.resolve(&cidfont, b"CIDToGIDMap").as_deref() {
                None => c.add("31-005", self.finding("Font {} on page {p} has no CIDToGIDMap", Vec::new())),
                Some(Object::Stream(_)) => {}
                Some(Object::Name(n)) if n.as_slice() == b"Identity" => {}
                Some(_) => c.add("31-004", self.finding("Font {} on page {p}: its CIDToGIDMap is neither a stream nor Identity", Vec::new())),
            }
        }
        self.cid_set(face, program, c);
    }

    /// The glyph id a TrueType CIDFont's CID selects; `None` past the end of its map.
    fn gid_of(&self, cid: u32) -> Option<u32> {
        match &self.gid_map {
            GidMap::Identity => Some(cid),
            GidMap::Table(t) => t.get(usize::try_from(cid).ok()?).map(|g| u32::from(*g)),
            GidMap::Unknown => None,
        }
    }

    /// Whether the CIDFont's glyphs are TrueType outlines, reached through CIDToGIDMap.
    fn truetype_cids(&self, face: &Face) -> bool {
        !face.cff_outlines() && self.cidfont.as_ref().and_then(|d| d.name(b"Subtype")) == Some(b"CIDFontType2")
    }

    /// 31-014 and 31-015: a CIDSet lists exactly the CIDs the program has glyphs for. CID 0 is
    /// left out both ways. A glyph a subsetter emptied doesn't count as one the program has, so
    /// a CIDSet may leave it out; one the CIDSet lists is there if the program has its glyph id.
    fn cid_set(&self, face: Option<&Face>, program: &Result<Option<Program>, Unreadable>, c: &mut Collected) {
        let Some(set) = self.descriptor.as_ref().and_then(|d| self.resolve(d, b"CIDSet")) else { return };
        let Object::Stream(set) = set.as_ref() else { return };
        let (bits, face) = match (set.decoded_within(MAX_CIDS / 8), face) {
            (Ok(bits), Some(face)) => (bits, face),
            // No program to hold the CIDSet against: 31-009 is about that.
            (Ok(_), None) if program.is_ok() => return,
            _ => {
                c.undecided("31-014");
                c.undecided("31-015");
                return;
            }
        };
        let listed = |cid: u32| usize::try_from(cid / 8).ok().and_then(|i| bits.get(i)).is_some_and(|b| b & (0x80 >> (cid % 8)) != 0);
        // Whether the program has CID `cid`, and whether its glyph has an outline.
        let has = |cid: u32| -> (bool, bool) {
            if face.cid_keyed() {
                let found = face.glyph_of_cid(cid).is_some();
                return (found, found);
            }
            let gid = if self.truetype_cids(face) { self.gid_of(cid) } else { Some(cid) };
            match gid {
                Some(g) if g != 0 && g < face.num_glyphs() => (true, face.has_outline(g)),
                _ => (false, false),
            }
        };
        let cids: Box<dyn Iterator<Item = u32>> = if face.cid_keyed() {
            let mut all: Vec<u32> = face.cids().collect();
            all.sort_unstable();
            Box::new(all.into_iter())
        } else {
            let last = match (&self.gid_map, self.truetype_cids(face)) {
                (GidMap::Table(t), true) => u32::try_from(t.len()).unwrap_or(u32::MAX),
                _ => face.num_glyphs(),
            };
            Box::new(0..last.min(MAX_CIDS as u32))
        };
        if let Some(cid) = cids.filter(|&cid| cid != 0).find(|&cid| has(cid).1 && !listed(cid)) {
            c.add(
                "31-014",
                self.finding("Font {} on page {p}: its CIDSet leaves out CID {cid}, which the font program has", vec![("{cid}", cid.to_string())]),
            );
        }
        let bit_count = u32::try_from(bits.len().saturating_mul(8)).unwrap_or(u32::MAX);
        if let Some(cid) = (1..bit_count).find(|&cid| listed(cid) && !has(cid).0) {
            c.add(
                "31-015",
                self.finding("Font {} on page {p}: its CIDSet lists CID {cid}, which the font program lacks", vec![("{cid}", cid.to_string())]),
            );
        }
    }

    /// 31-017 to 31-026: a simple TrueType font's encoding and cmaps (ISO 14289-1 7.21.6).
    fn truetype(&self, face: Option<&Face>, program: &Result<Option<Program>, Unreadable>, c: &mut Collected) {
        let encoding = self.resolve(self.dict, b"Encoding");
        let unreadable = program.is_err();
        let cmaps = face.map(Face::cmaps).unwrap_or_default();
        let rendered = self.used.is_some_and(|u| u.values().any(|u| u.rendered));
        let latin = |n: &[u8]| matches!(n, b"MacRomanEncoding" | b"WinAnsiEncoding");
        if self.symbolic {
            if encoding.is_some() {
                c.add("31-024", self.finding("Font {} on page {p} is a symbolic TrueType font with an Encoding entry", Vec::new()));
            }
            if unreadable {
                c.undecided("31-025");
                c.undecided("31-026");
            } else if face.is_some() {
                if cmaps.is_empty() {
                    c.add("31-025", self.finding("Font {} on page {p}: its TrueType program has no cmap", Vec::new()));
                } else if cmaps.len() > 1 && !cmaps.contains(&(3, 0)) {
                    c.add("31-026", self.finding("Font {} on page {p}: its TrueType program has several cmaps but no Symbol (3,0) one", Vec::new()));
                }
            }
            return;
        }
        let mut differences = false;
        match encoding.as_deref() {
            None => c.add("31-019", self.finding("Font {} on page {p} is a non-symbolic TrueType font with no Encoding", Vec::new())),
            Some(Object::Name(n)) if !latin(n) => {
                c.add(
                    "31-021",
                    self.finding(
                        "Font {} on page {p}: its encoding {enc} is neither MacRomanEncoding nor WinAnsiEncoding",
                        vec![("{enc}", pdf_name(n))],
                    ),
                );
            }
            Some(Object::Dict(d)) => {
                match d.name(b"BaseEncoding") {
                    None => c.add("31-020", self.finding("Font {} on page {p}: its Encoding dictionary has no BaseEncoding", Vec::new())),
                    Some(n) if !latin(n) => c.add(
                        "31-021",
                        self.finding(
                            "Font {} on page {p}: its encoding {enc} is neither MacRomanEncoding nor WinAnsiEncoding",
                            vec![("{enc}", pdf_name(n))],
                        ),
                    ),
                    Some(_) => {}
                }
                let items = self.resolve(d, b"Differences").and_then(|x| x.as_array().cloned()).unwrap_or_default();
                let names: Vec<String> = items.iter().filter_map(|x| self.doc.resolve(x).as_name().map(pdf_name)).collect();
                differences = !names.is_empty();
                for name in names {
                    match listed(&name) {
                        Listed::Yes => {}
                        Listed::Unknown => c.undecided("31-022"),
                        Listed::No => c.add(
                            "31-022",
                            self.finding(
                                "Font {} on page {p}: its Differences name glyph {glyph}, which isn't in the Adobe Glyph List",
                                vec![("{glyph}", name)],
                            ),
                        ),
                    }
                }
            }
            Some(_) => {}
        }
        if unreadable {
            if differences {
                c.undecided("31-023");
            }
            if rendered {
                c.undecided("31-017");
                c.undecided("31-018");
            }
            return;
        }
        let Some(face) = face else { return };
        if differences && !cmaps.contains(&(3, 1)) {
            c.add("31-023", self.finding("Font {} on page {p} has Differences, but its program has no Unicode (3,1) cmap", Vec::new()));
        }
        if !rendered {
            return;
        }
        if !cmaps.contains(&(3, 1)) && !cmaps.contains(&(1, 0)) {
            c.add("31-017", self.finding("Font {} on page {p}: its TrueType program has no Unicode (3,1) or Mac Roman (1,0) cmap", Vec::new()));
            return;
        }
        for (&code, u) in self.used.into_iter().flatten().filter(|(_, u)| u.rendered) {
            match self.nonsymbolic_cmap_glyph(code, face) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Named::Is(name) = self.named(code) {
                        let values = vec![("{glyph}", name.clone()), ("{code}", self.code_text(code))];
                        c.add(
                            "31-018",
                            self.finding_at(
                                u.page,
                                "Font {} on page {p}: no Unicode or Mac Roman cmap of its program finds glyph {glyph} (code {code})",
                                values,
                            ),
                        );
                        break;
                    }
                }
                Err(Undecided) => c.undecided("31-018"),
            }
        }
    }

    /// The glyph a non-symbolic TrueType font's code selects through its non-symbolic cmaps
    /// (ISO 32000-1 §9.6.6.4): its name through the Adobe Glyph List to a (3,1) cmap, or through
    /// Mac OS Roman to a (1,0) one. `Ok(None)` when neither finds one; `Err` when the name is one
    /// PdfCraft can't take to Unicode and the (1,0) cmap didn't settle it.
    fn nonsymbolic_cmap_glyph(&self, code: u32, face: &Face) -> Result<Option<u32>, Undecided> {
        let name = match self.named(code) {
            Named::Is(n) => n,
            Named::Undefined => return Ok(None),
            Named::Builtin | Named::Unknown => return Err(Undecided),
        };
        let mut undecided = false;
        if face.cmaps().contains(&(3, 1)) {
            match glyph_unicode(name) {
                Some(u) => {
                    if let Some(g) = face.cmap_glyph(3, 1, u32::from(u)) {
                        return Ok(Some(g));
                    }
                }
                None => undecided = true,
            }
        }
        if let Some(mac) = BaseEncoding::MacRoman.code_of(name)
            && let Some(g) = face.cmap_glyph(1, 0, u32::from(mac))
        {
            return Ok(Some(g));
        }
        if undecided { Err(Undecided) } else { Ok(None) }
    }

    /// 31-012 and 31-013: a Type 1 font's CharSet lists exactly the glyphs its program has,
    /// .notdef aside.
    fn charset(&self, face: Option<&Face>, program: &Result<Option<Program>, Unreadable>, c: &mut Collected) {
        let Some(charset) = self.descriptor.as_ref().and_then(|d| self.resolve(d, b"CharSet")) else { return };
        let Some(charset) = charset.as_string().map(|s| s.bytes.clone()) else { return };
        if program.is_err() {
            c.undecided("31-012");
            c.undecided("31-013");
            return;
        }
        let Some(face) = face else { return };
        let listed: BTreeSet<String> =
            charset.split(|b| *b == b'/').map(|n| pdf_name(n.trim_ascii())).filter(|n| !n.is_empty() && n != ".notdef").collect();
        let has: BTreeSet<&str> = face.glyph_names().filter(|n| *n != ".notdef").collect();
        if let Some(glyph) = has.iter().find(|n| !listed.contains(**n)) {
            c.add(
                "31-012",
                self.finding(
                    "Font {} on page {p}: its CharSet leaves out glyph {glyph}, which the font program has",
                    vec![("{glyph}", (*glyph).to_owned())],
                ),
            );
        }
        if let Some(glyph) = listed.iter().find(|n| !has.contains(n.as_str())) {
            c.add(
                "31-013",
                self.finding("Font {} on page {p}: its CharSet lists glyph {glyph}, which the font program lacks", vec![("{glyph}", glyph.clone())]),
            );
        }
    }

    /// What `code` selects (ISO 32000-1 §9.6.6 and §9.7.4).
    fn glyph(&self, code: u32, face: Option<&Face>) -> Glyph {
        match self.kind {
            Kind::Type0 => self.cid_glyph(code, face),
            Kind::Type1 => self.type1_glyph(code, face),
            Kind::TrueType => self.truetype_glyph(code, face),
            Kind::Type3 => match self.named(code) {
                Named::Is(n) if n == ".notdef" => Glyph::Notdef,
                Named::Is(n) => {
                    let procs = self.resolve(self.dict, b"CharProcs").and_then(|p| p.as_dict().cloned()).unwrap_or_default();
                    if procs.contains(n.as_bytes()) { Glyph::Found(0) } else { Glyph::Missing }
                }
                Named::Undefined => Glyph::Notdef,
                Named::Builtin | Named::Unknown => Glyph::Unknown,
            },
            Kind::Other => Glyph::Unknown,
        }
    }

    fn type1_glyph(&self, code: u32, face: Option<&Face>) -> Glyph {
        match self.named(code) {
            Named::Is(n) if n == ".notdef" => Glyph::Notdef,
            Named::Is(n) => match face {
                Some(f) => f.glyph_of_name(n).map_or(Glyph::Missing, Glyph::Found),
                None => Glyph::Named,
            },
            Named::Undefined => Glyph::Notdef,
            Named::Builtin => match (face, u8::try_from(code)) {
                // Glyph 0 is .notdef in a CFF program, and read-fonts puts it first in a Type 1 one.
                (Some(f), Ok(byte)) => match f.builtin_glyph(byte) {
                    Some(0) | None => Glyph::Notdef,
                    Some(g) => Glyph::Found(g),
                },
                _ => Glyph::Unknown,
            },
            Named::Unknown => Glyph::Unknown,
        }
    }

    fn truetype_glyph(&self, code: u32, face: Option<&Face>) -> Glyph {
        let Some(face) = face else {
            return if !self.symbolic && *self.named(code) == Named::Undefined { Glyph::Notdef } else { Glyph::Unknown };
        };
        let cmaps = face.cmaps();
        if cmaps.is_empty() {
            return Glyph::Unknown;
        }
        let found = |g: u32| if g < face.num_glyphs() { Glyph::Found(g) } else { Glyph::Missing };
        if self.symbolic {
            // A (3,0) cmap at the code, or at it in one of the ranges F000, F100 or F200; a (1,0)
            // cmap at the code (§9.6.6.4). Past those, a reader may choose its own mapping, and
            // most read the code through whatever cmap there is.
            let tries = [(3u16, 0u16, code), (3, 0, 0xF000 | code), (3, 0, 0xF100 | code), (3, 0, 0xF200 | code), (1, 0, code)];
            let others = cmaps.iter().map(|&(p, e)| (p, e, code));
            return tries.into_iter().chain(others).find_map(|(p, e, cp)| face.cmap_glyph(p, e, cp)).map_or(Glyph::Notdef, found);
        }
        let through_cmaps = self.nonsymbolic_cmap_glyph(code, face);
        if let Ok(Some(g)) = through_cmaps {
            return found(g);
        }
        // Then the name in the `post` table, then (as readers do) the code in a (3,0) cmap.
        if let Named::Is(n) = self.named(code)
            && let Some(g) = face.glyph_of_name(n)
        {
            return found(g);
        }
        if let Some(g) = [code, 0xF000 | code].into_iter().find_map(|cp| face.cmap_glyph(3, 0, cp)) {
            return found(g);
        }
        if through_cmaps.is_err() { Glyph::Unknown } else { Glyph::Notdef }
    }

    fn cid_glyph(&self, code: u32, face: Option<&Face>) -> Glyph {
        if !self.cids_known {
            return Glyph::Unknown;
        }
        let cid = self.metrics.cid(code);
        if cid == 0 {
            return Glyph::Notdef;
        }
        let truetype = self.cidfont.as_ref().and_then(|d| d.name(b"Subtype")) == Some(b"CIDFontType2");
        let Some(face) = face else {
            // No program: a CID other than 0 names a glyph other than .notdef, unless a TrueType
            // CIDFont's map sends it to glyph 0.
            return match (truetype, &self.gid_map) {
                (true, GidMap::Unknown) => Glyph::Unknown,
                (true, _) if self.gid_of(cid).unwrap_or(0) == 0 => Glyph::Notdef,
                _ => Glyph::Named,
            };
        };
        if face.cid_keyed() {
            return face.glyph_of_cid(cid).map_or(Glyph::Missing, Glyph::Found);
        }
        let gid = if self.truetype_cids(face) {
            match (&self.gid_map, self.gid_of(cid)) {
                (GidMap::Unknown, _) => return Glyph::Unknown,
                (_, gid) => gid.unwrap_or(0),
            }
        } else {
            cid
        };
        match gid {
            0 => Glyph::Notdef,
            g if g < face.num_glyphs() => Glyph::Found(g),
            _ => Glyph::Missing,
        }
    }

    /// The width a Type 3 glyph's description gives, in text space per unit of font size: the
    /// `wx` of its `d0` or `d1`, through the FontMatrix.
    fn type3_width(&self, code: u32) -> Option<f64> {
        let Named::Is(name) = self.named(code) else { return None };
        let procs = self.resolve(self.dict, b"CharProcs").and_then(|p| p.as_dict().cloned())?;
        let proc = procs.get(name.as_bytes()).map(|p| self.doc.resolve(p))?;
        let Object::Stream(proc) = proc.as_ref() else { return None };
        let ops = pdfcraft_content::parse(&proc.decoded_within(MAX_STREAM).ok()?).ops;
        let wx = ops.iter().find(|op| matches!(op.op.as_slice(), b"d0" | b"d1"))?.operands.first()?.as_f64()?;
        let matrix = self.resolve(self.dict, b"FontMatrix").and_then(|m| m.as_array().cloned())?;
        let scale = matrix.first().and_then(|v| self.doc.resolve(v).as_f64())?;
        let width = wx * scale;
        width.is_finite().then_some(width)
    }

    /// 31-011, 31-016 and 31-030: the glyphs the content selects, and their widths.
    fn glyphs(&self, face: Option<&Face>, program: &Result<Option<Program>, Unreadable>, c: &mut Collected) {
        let Some(used) = self.used else { return };
        let unreadable = program.is_err();
        let embedded = matches!(program, Ok(Some(_))) || self.kind == Kind::Type3;
        // A simple font gives every code its width in /Widths; without it there's nothing of the
        // dictionary's to hold the program's widths against.
        let widths = self.kind == Kind::Type0 || self.dict.get(b"Widths").is_some();
        let (mut missing, mut notdef, mut wide) = (false, false, false);
        for (&code, u) in used {
            let glyph = if unreadable { Glyph::Unknown } else { self.glyph(code, face) };
            match glyph {
                Glyph::Notdef if !notdef => {
                    notdef = true;
                    c.add(
                        "31-030",
                        self.finding_at(u.page, "Font {} on page {p} draws the .notdef glyph (code {code})", vec![("{code}", self.code_text(code))]),
                    );
                }
                Glyph::Unknown => c.undecided("31-030"),
                _ => {}
            }
            if !u.rendered || !(embedded || unreadable) {
                continue;
            }
            match glyph {
                Glyph::Missing if !missing => {
                    missing = true;
                    c.add(
                        "31-011",
                        self.finding_at(
                            u.page,
                            "Font {} on page {p} draws a glyph its font program lacks (code {code})",
                            vec![("{code}", self.code_text(code))],
                        ),
                    );
                }
                Glyph::Unknown | Glyph::Named => {
                    c.undecided("31-011");
                    c.undecided("31-016");
                }
                Glyph::Found(gid) if !wide => {
                    let program_width = match self.kind {
                        Kind::Type3 => self.type3_width(code),
                        _ => face.and_then(|f| f.advance(gid)).map(|w| w / 1000.0),
                    };
                    match program_width {
                        Some(pw) if widths => {
                            let dw = self.metrics.width(code);
                            if (dw - pw).abs() > WIDTH_TOLERANCE {
                                wide = true;
                                let values = vec![("{code}", self.code_text(code)), ("{w}", thousandths(dw)), ("{fw}", thousandths(pw))];
                                c.add(
                                    "31-016",
                                    self.finding_at(
                                        u.page,
                                        "Font {} on page {p}: code {code} is {w} wide in the font dictionary but {fw} in the font program",
                                        values,
                                    ),
                                );
                            }
                        }
                        _ => c.undecided("31-016"),
                    }
                }
                _ => {}
            }
        }
    }

    /// The Type 0 font's CIDs have published Unicode values: its CIDFont is of Adobe-GB1,
    /// Adobe-CNS1, Adobe-Japan1 or Adobe-Korea1.
    fn known_collection(&self) -> bool {
        let Some(info) = self.cidfont.as_ref().and_then(|d| self.system_info(d)) else { return false };
        info.registry.as_deref() == Some(b"Adobe") && info.ordering.is_some_and(|o| KNOWN_COLLECTIONS.contains(&o.as_slice()))
    }

    /// The CIDSystemInfo of a CMap stream's or CIDFont's dictionary, when it has one.
    fn system_info(&self, d: &Dict) -> Option<SystemInfo> {
        let info = self.resolve(d, b"CIDSystemInfo").and_then(|i| i.as_dict().cloned())?;
        let text = |key: &[u8]| self.resolve(&info, key).and_then(|v| v.as_string().map(|s| s.bytes.clone()));
        Some(SystemInfo {
            registry: text(b"Registry"),
            ordering: text(b"Ordering"),
            supplement: self.resolve(&info, b"Supplement").and_then(|v| v.as_int()),
        })
    }

    /// The glyph name `code` selects, when the encoding or the program's built-in one gives one.
    fn glyph_name(&self, code: u32, face: Option<&Face>) -> Option<String> {
        match self.named(code) {
            Named::Is(n) => Some(n.clone()),
            Named::Builtin => face?.glyph_name(face?.builtin_glyph(u8::try_from(code).ok()?)?),
            Named::Undefined | Named::Unknown => None,
        }
    }

    /// 31-027 to 31-029: a ToUnicode map, its values, and when a font may go without one.
    fn to_unicode(&self, face: Option<&Face>, c: &mut Collected) {
        if let Some(Object::Stream(map)) = self.resolve(self.dict, b"ToUnicode").as_deref() {
            // Read within the bound `Metrics` reads it within: a larger map has no codes there to
            // look up, so what it maps can't be decided.
            if map.decoded_within(MAX_CMAP_BYTES).is_err() {
                c.undecided("31-028");
                c.undecided("31-029");
                return;
            }
            if let Some(code) = self.metrics.to_unicode_code_with(0) {
                c.add(
                    "31-028",
                    self.finding("Font {} on page {p}: its ToUnicode map maps code {code} to U+0000", vec![("{code}", self.code_text(code))]),
                );
            }
            for (unit, said) in [(0xFEFF, "U+FEFF"), (0xFFFE, "U+FFFE")] {
                if let Some(code) = self.metrics.to_unicode_code_with(unit) {
                    let values = vec![("{code}", self.code_text(code)), ("{u}", said.to_owned())];
                    c.add("31-029", self.finding("Font {} on page {p}: its ToUnicode map maps code {code} to {u}", values));
                }
            }
            return;
        }
        match self.needs_no_to_unicode(face) {
            Some(true) => {}
            Some(false) => {
                c.add("31-027", self.finding("Font {} on page {p} has no ToUnicode map, and no exception lets it go without one", Vec::new()))
            }
            None => c.undecided("31-027"),
        }
    }

    /// Whether one of 31-027's exceptions lets this font go without a ToUnicode map; `None` when
    /// that turns on glyph names PdfCraft can't place.
    fn needs_no_to_unicode(&self, face: Option<&Face>) -> Option<bool> {
        let encoding = self.resolve(self.dict, b"Encoding");
        let predefined_encoding = matches!(encoding.as_deref(), Some(Object::Name(n)) if matches!(n.as_slice(), b"MacRomanEncoding" | b"MacExpertEncoding" | b"WinAnsiEncoding"));
        match self.kind {
            Kind::Type0 => Some(self.known_collection()),
            Kind::TrueType => Some(!self.symbolic || predefined_encoding),
            Kind::Type1 | Kind::Type3 if predefined_encoding => Some(true),
            Kind::Type1 | Kind::Type3 => {
                // Every glyph the text uses is named from the Latin set or the Symbol font's set.
                let symbol_font = self.dict.name(b"BaseFont").and_then(|b| Std14::from_base_font(&pdf_name(b))) == Some(Std14::Symbol);
                let mut unknown = false;
                for &code in self.used.into_iter().flat_map(BTreeMap::keys) {
                    match self.glyph_name(code, face) {
                        // A code naming .notdef names no character; 31-030 is about that.
                        Some(n) if latin_glyph(&n) || n == ".notdef" => {}
                        // The Symbol set's names are letters and digits; anything else is in neither set.
                        Some(n) if !n.bytes().all(|b| b.is_ascii_alphanumeric()) => return Some(false),
                        Some(_) => unknown = true,
                        None if *self.named(code) == Named::Undefined => {}
                        None if symbol_font => {}
                        None => unknown = true,
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Kind::Other => Some(false),
        }
    }

    /// 17-003: each code a formula shows maps to Unicode, through the ToUnicode map, the glyph
    /// name the encoding or the program gives it, or (Type 0) a character collection with
    /// published Unicode values.
    fn formula(&self, codes: &BTreeMap<u32, usize>, face: Option<&Face>, c: &mut Collected) {
        for (&code, &page) in codes {
            let mapped = self.metrics.text_of(code).is_some_and(|t| !t.is_empty() && !t.contains(['\u{FFFD}', '\0']));
            let maps = match self.kind {
                _ if mapped => Some(true),
                Kind::Type0 if self.known_collection() => Some(true),
                Kind::Type0 if self.cids_known => Some(false),
                // Codes PdfCraft can't take to CIDs here may yet map.
                Kind::Type0 => None,
                Kind::Other => Some(false),
                // A glyph name PdfCraft can't take to Unicode may yet have a value in the Adobe
                // Glyph List; a code with no name, or .notdef, has none.
                _ => match self.glyph_name(code, face) {
                    Some(n) if n == ".notdef" => Some(false),
                    Some(n) if glyph_text(&n).is_some() => Some(true),
                    Some(_) => None,
                    None if *self.named(code) == Named::Undefined => Some(false),
                    None => None,
                },
            };
            match maps {
                Some(true) => {}
                Some(false) => {
                    c.add(
                        "17-003",
                        self.finding_at(
                            page,
                            "Font {} on page {p}: code {code} in a formula can't be mapped to Unicode",
                            vec![("{code}", self.code_text(code))],
                        ),
                    );
                    return;
                }
                None => c.undecided("17-003"),
            }
        }
    }
}

/// Whether a glyph name is in the Adobe Glyph List.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listed {
    Yes,
    No,
    /// It might be: PdfCraft doesn't hold the list.
    Unknown,
}

/// A name of the Latin set, or one the Adobe Glyph List's rules spell, counts as listed. Its
/// entries are letters and digits, starting with a letter, so a name of anything else isn't.
fn listed(name: &str) -> Listed {
    if latin_glyph(name) || glyph_text(name).is_some() {
        Listed::Yes
    } else if name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()) && name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Listed::Unknown
    } else {
        Listed::No
    }
}

/// A width in text space per unit of font size, as thousandths, to two places.
fn thousandths(w: f64) -> String {
    let v = (w * 100_000.0).round() / 100.0;
    format!("{}", if v == 0.0 { 0.0 } else { v })
}
