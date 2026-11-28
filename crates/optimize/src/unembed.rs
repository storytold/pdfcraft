//! Unembed fonts: take the embedded program out of simple fonts a reader can substitute from the
//! font dictionary alone (ISO 32000-2 §9.5, §9.8.1), to make the file smaller.
//!
//! Only a Type 1, multiple master or TrueType font whose codes reach glyphs through a standard
//! encoding is unembedded. Every other font is listed with the reason it stays:
//! - Type 3 fonts draw with content streams, and composite fonts reach glyphs by CID, which no
//!   installed font is guaranteed to share;
//! - a symbolic font without a standard encoding uses the program's own built-in encoding, which
//!   leaves with the program;
//! - a font without `/Widths` would be laid out with the substitute's widths;
//! - a font written directly in a resource dictionary, or whose descriptor is written directly in
//!   the font, or is also used by a font that stays, can't be changed alone.
//!
//! An unembedded font keeps its widths, descriptor metrics, `/Encoding` and `/ToUnicode`, so text
//! keeps its place and stays searchable; content streams are not touched. The program key, and
//! `/CIDSet` and `/CharSet` (which describe the program's glyphs), are removed. The subset tag
//! (§9.9.2) is taken off `/BaseFont` and the descriptor's `/FontName` (which must match, §9.8.1)
//! unless asked to keep it, since a tagged name matches no installed font.
//!
//! The program itself goes on the next full save, once nothing refers to it; one shared with a
//! font that stays is kept, and not counted as freed. PDF/A and PDF/X require embedded fonts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OptimizeError;

/// The keys of a descriptor that hold a font program (§9.9.1); at most one is present.
const PROGRAM_KEYS: [&[u8]; 3] = [b"FontFile", b"FontFile2", b"FontFile3"];

/// Encodings a substitute font maps codes through without the program (Annex D).
const STANDARD_ENCODINGS: [&[u8]; 4] = [b"WinAnsiEncoding", b"MacRomanEncoding", b"MacExpertEncoding", b"StandardEncoding"];

/// The descriptor flag for a font whose glyphs are outside the standard Latin set (§9.8.2).
const SYMBOLIC: i64 = 1 << 2;

/// Which fonts to unembed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// The fonts by `/BaseFont`, with or without its subset tag (case-sensitive); `None` for every
    /// font that can be unembedded.
    pub fonts: Option<Vec<String>>,
    /// Leave subset tags on the names.
    pub keep_subset_tag: bool,
}

/// Why a font keeps its program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Type3,
    Composite,
    SymbolicEncoding,
    NoWidths,
    ProgramUnreadable,
    UnknownSubtype,
    FontNotIndirect,
    DescriptorNotIndirect,
    DescriptorShared,
}

impl Refusal {
    /// A stable token for scripts.
    pub fn token(self) -> &'static str {
        match self {
            Self::Type3 => "type3",
            Self::Composite => "composite",
            Self::SymbolicEncoding => "symbolic-encoding",
            Self::NoWidths => "no-widths",
            Self::ProgramUnreadable => "program-unreadable",
            Self::UnknownSubtype => "unknown-subtype",
            Self::FontNotIndirect => "font-not-indirect",
            Self::DescriptorNotIndirect => "descriptor-not-indirect",
            Self::DescriptorShared => "descriptor-shared",
        }
    }

    /// The reason in words.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Type3 => "a Type 3 font draws its glyphs with content in the file",
            Self::Composite => "a composite font reaches its glyphs by CID, which a substitute need not share",
            Self::SymbolicEncoding => "a symbolic font without a standard encoding uses the program's own encoding",
            Self::NoWidths => "without /Widths the text would take the substitute's widths",
            Self::ProgramUnreadable => "its font program or descriptor can't be read",
            Self::UnknownSubtype => "its font type isn't one that can be substituted",
            Self::FontNotIndirect => "the font is written directly in a resource dictionary",
            Self::DescriptorNotIndirect => "its descriptor is written directly in the font",
            Self::DescriptorShared => "its descriptor is also used by a font that keeps its program",
        }
    }
}

/// What happens to one font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Unembed,
    NotEmbedded,
    /// A list of names was given without this font's.
    NotSelected,
    Refused(Refusal),
}

/// One font the document uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPlan {
    /// The font dictionary; `None` when it is written directly in a resource dictionary.
    pub font: Option<ObjRef>,
    /// `/BaseFont` as it is now.
    pub name: String,
    /// `/Subtype`.
    pub subtype: String,
    pub outcome: Outcome,
    /// The name without its subset tag, when unembedding takes the tag off.
    pub renamed_to: Option<String>,
    /// The stored size of the embedded program (0 when there is none).
    pub program_bytes: u64,
    /// Nothing else refers to the program once unembedded, so a full save drops it.
    pub freed: bool,
}

/// What [`unembed`] does, or did, to each font.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Every font the document uses, indirect ones by object number, then direct ones.
    pub fonts: Vec<FontPlan>,
    /// Requested names no font has.
    pub unmatched: Vec<String>,
    /// The stored size of the programs a full save drops, each counted once.
    pub bytes_freed: u64,
}

impl Plan {
    /// How many fonts are unembedded.
    pub fn unembedded(&self) -> usize {
        self.fonts.iter().filter(|f| f.outcome == Outcome::Unembed).count()
    }
}

/// `name` without a subset tag: six uppercase letters and `+` (§9.9.2).
pub fn strip_subset_tag(name: &str) -> &str {
    let b = name.as_bytes();
    let tagged = b.len() > 7 && b.get(6) == Some(&b'+') && b.get(..6).is_some_and(|t| t.iter().all(u8::is_ascii_uppercase));
    if tagged { name.get(7..).unwrap_or(name) } else { name }
}

/// What [`unembed`] would do, without changing the document.
pub fn plan(doc: &Document, request: &Request) -> Plan {
    build(doc, request).0
}

/// Unembed the fonts `request` selects, and report on every font. Fonts that can't be unembedded
/// are reported, not an error.
pub fn unembed(doc: &mut Document, request: &Request) -> Result<Plan, OptimizeError> {
    let (plan, edits) = build(doc, request);
    for e in &edits {
        if let Some(name) = &e.rename {
            doc.update_dict(e.font, |d| d.set(b"BaseFont".to_vec(), Object::Name(name.as_bytes().to_vec())))?;
        }
        let keep = request.keep_subset_tag;
        doc.update_dict(e.descriptor, |d| {
            for key in PROGRAM_KEYS.iter().chain([&b"CIDSet"[..], b"CharSet"].iter()) {
                d.remove(key);
            }
            let name = d.name(b"FontName").map(|n| String::from_utf8_lossy(n).into_owned());
            if let (false, Some(name)) = (keep, name) {
                let bare = strip_subset_tag(&name);
                if bare != name {
                    d.set(b"FontName".to_vec(), Object::Name(bare.as_bytes().to_vec()));
                }
            }
        })?;
    }
    Ok(plan)
}

/// A font that can be unembedded: where its program is.
#[derive(Clone, Copy)]
struct Candidate {
    descriptor: ObjRef,
    program: ObjRef,
    bytes: u64,
}

/// A change [`unembed`] makes.
struct Change {
    font: ObjRef,
    descriptor: ObjRef,
    rename: Option<String>,
}

/// The fonts the document uses, and which objects refer to each object.
#[derive(Default)]
struct Census {
    /// For each object the trailer reaches, the objects whose values refer to it (`None` for
    /// the trailer).
    referrers: HashMap<u32, HashSet<Option<u32>>>,
    fonts: BTreeSet<ObjRef>,
    /// Fonts written directly in a resource dictionary.
    direct: Vec<Dict>,
}

/// How a value is used where it appears.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// A `/Font` resource dictionary: its values are fonts.
    FontEntries,
    Font,
    Other,
}

fn census(doc: &Document) -> Census {
    let child = |key: &[u8], parent: Use| match (parent, key) {
        (Use::FontEntries, _) => Use::Font,
        (_, b"Font") => Use::FontEntries,
        _ => Use::Other,
    };
    let mut c = Census::default();
    let mut seen = HashSet::new();
    let mut stack: Vec<(Object, Option<u32>, Use)> = doc.trailer().iter().map(|(k, v)| (v.clone(), None, child(k, Use::Other))).collect();
    while let Some((obj, holder, how)) = stack.pop() {
        match obj {
            Object::Ref(r) => {
                c.referrers.entry(r.num).or_default().insert(holder);
                let target = doc.get(r);
                let dict = match &*target {
                    Object::Dict(d) => Some(d),
                    Object::Stream(s) => Some(&s.dict),
                    _ => None,
                };
                if dict.is_some_and(|d| (how == Use::Font && d.contains(b"Subtype")) || is_font(d)) {
                    c.fonts.insert(r);
                }
                if !seen.insert(r.num) {
                    continue;
                }
                match &*target {
                    Object::Dict(d) => stack.extend(d.iter().map(|(k, v)| (v.clone(), Some(r.num), child(k, how)))),
                    Object::Stream(s) => stack.extend(s.dict.iter().map(|(k, v)| (v.clone(), Some(r.num), child(k, how)))),
                    Object::Array(a) => stack.extend(a.iter().map(|v| (v.clone(), Some(r.num), how))),
                    _ => {}
                }
            }
            Object::Dict(d) => {
                if how == Use::Font && d.contains(b"Subtype") {
                    c.direct.push(d.clone());
                }
                stack.extend(d.iter().map(|(k, v)| (v.clone(), holder, child(k, how))));
            }
            Object::Array(a) => stack.extend(a.into_iter().map(|v| (v, holder, how))),
            _ => {}
        }
    }
    c
}

/// A top-level font dictionary (descendant CIDFonts belong to their Type 0 font).
fn is_font(d: &Dict) -> bool {
    d.name(b"Type") == Some(b"Font") && !matches!(d.name(b"Subtype"), Some(b"CIDFontType0" | b"CIDFontType2"))
}

fn text(d: &Dict, key: &[u8]) -> String {
    d.name(key).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()
}

/// Whether `font` can be unembedded, and where its program is.
fn classify(doc: &Document, font: &Dict) -> Result<Candidate, Outcome> {
    let refused = |r| Err(Outcome::Refused(r));
    match font.name(b"Subtype") {
        Some(b"Type3") => return refused(Refusal::Type3),
        Some(b"Type0" | b"CIDFontType0" | b"CIDFontType2") => return refused(Refusal::Composite),
        Some(b"Type1" | b"MMType1" | b"TrueType") => {}
        _ => return refused(Refusal::UnknownSubtype),
    }
    let descriptor = match font.get(b"FontDescriptor") {
        None => return Err(Outcome::NotEmbedded),
        Some(Object::Ref(r)) => *r,
        Some(Object::Dict(d)) if PROGRAM_KEYS.iter().any(|k| d.contains(k)) => return refused(Refusal::DescriptorNotIndirect),
        Some(Object::Dict(_)) => return Err(Outcome::NotEmbedded),
        Some(_) => return refused(Refusal::ProgramUnreadable),
    };
    let fd = doc.get(descriptor);
    let Some(fd) = fd.as_dict() else { return refused(Refusal::ProgramUnreadable) };
    let Some(value) = PROGRAM_KEYS.iter().find_map(|k| fd.get(k)) else { return Err(Outcome::NotEmbedded) };
    let Some(program) = value.as_ref() else { return refused(Refusal::ProgramUnreadable) };
    let bytes = match &*doc.get(program) {
        Object::Stream(s) => s.raw.len() as u64,
        _ => return refused(Refusal::ProgramUnreadable),
    };
    if doc.resolve(font.get(b"Widths").unwrap_or(&Object::Null)).as_array().is_none() {
        return refused(Refusal::NoWidths);
    }
    let symbolic = fd.get(b"Flags").map(|f| doc.resolve(f)).and_then(|f| f.as_int()).is_some_and(|f| f & SYMBOLIC != 0);
    if symbolic && !standard_encoding(doc, font.get(b"Encoding")) {
        return refused(Refusal::SymbolicEncoding);
    }
    Ok(Candidate { descriptor, program, bytes })
}

/// A standard encoding, by name or as an encoding dictionary's `/BaseEncoding` (§9.6.5).
fn standard_encoding(doc: &Document, encoding: Option<&Object>) -> bool {
    let Some(e) = encoding.map(|e| doc.resolve(e)) else { return false };
    let base = match &*e {
        Object::Name(n) => Some(n.as_slice()),
        Object::Dict(d) => d.name(b"BaseEncoding"),
        _ => None,
    };
    base.is_some_and(|b| STANDARD_ENCODINGS.contains(&b))
}

fn selected(name: &str, request: &Request) -> bool {
    request.fonts.as_ref().is_none_or(|names| names.iter().any(|n| n == name || n == strip_subset_tag(name)))
}

/// Drop targets whose descriptor something other than a target refers to, until none is left.
fn settle(targets: &mut BTreeMap<ObjRef, Candidate>, referrers: &HashMap<u32, HashSet<Option<u32>>>) -> Vec<ObjRef> {
    let mut dropped = Vec::new();
    loop {
        let nums: HashSet<u32> = targets.keys().map(|r| r.num).collect();
        let shared: Vec<ObjRef> = targets
            .iter()
            .filter(|(_, c)| referrers.get(&c.descriptor.num).is_none_or(|from| from.iter().any(|h| !h.is_some_and(|h| nums.contains(&h)))))
            .map(|(f, _)| *f)
            .collect();
        if shared.is_empty() {
            return dropped;
        }
        for f in shared {
            targets.remove(&f);
            dropped.push(f);
        }
    }
}

fn build(doc: &Document, request: &Request) -> (Plan, Vec<Change>) {
    let c = census(doc);
    let mut fonts = Vec::new();
    let mut targets = BTreeMap::new();
    for &r in &c.fonts {
        let obj = doc.get(r);
        let Some(d) = obj.as_dict() else { continue };
        let entry = |outcome, bytes| FontPlan {
            font: Some(r),
            name: text(d, b"BaseFont"),
            subtype: text(d, b"Subtype"),
            outcome,
            renamed_to: None,
            program_bytes: bytes,
            freed: false,
        };
        match classify(doc, d) {
            Ok(cand) if selected(&text(d, b"BaseFont"), request) => {
                targets.insert(r, cand);
                fonts.push(entry(Outcome::Unembed, cand.bytes));
            }
            Ok(cand) => fonts.push(entry(Outcome::NotSelected, cand.bytes)),
            Err(outcome) => fonts.push(entry(outcome, 0)),
        }
    }
    let dropped = settle(&mut targets, &c.referrers);
    let descriptors: HashSet<u32> = targets.values().map(|t| t.descriptor.num).collect();
    let mut counted = HashSet::new();
    let mut bytes_freed = 0u64;
    let mut changes = Vec::new();
    for f in &mut fonts {
        let Some(r) = f.font else { continue };
        if dropped.contains(&r) {
            f.outcome = Outcome::Refused(Refusal::DescriptorShared);
            continue;
        }
        let Some(t) = targets.get(&r) else { continue };
        let from = c.referrers.get(&t.program.num);
        f.freed = from.is_some_and(|from| from.iter().all(|h| h.is_some_and(|h| descriptors.contains(&h))));
        if f.freed && counted.insert(t.program.num) {
            bytes_freed = bytes_freed.saturating_add(t.bytes);
        }
        let bare = strip_subset_tag(&f.name);
        if !request.keep_subset_tag && bare != f.name {
            f.renamed_to = Some(bare.to_string());
        }
        changes.push(Change { font: r, descriptor: t.descriptor, rename: f.renamed_to.clone() });
    }
    fonts.extend(c.direct.iter().map(|d| FontPlan {
        font: None,
        name: text(d, b"BaseFont"),
        subtype: text(d, b"Subtype"),
        outcome: match classify(doc, d) {
            Ok(_) => Outcome::Refused(Refusal::FontNotIndirect),
            Err(o) => o,
        },
        renamed_to: None,
        program_bytes: 0,
        freed: false,
    }));
    let all: Vec<&str> = fonts.iter().map(|f| f.name.as_str()).collect();
    let unmatched = request.fonts.iter().flatten().filter(|n| !all.iter().any(|a| a == n || strip_subset_tag(a) == n.as_str())).cloned().collect();
    (Plan { fonts, unmatched, bytes_freed }, changes)
}
