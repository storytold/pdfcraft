//! Text emission: Type 1 passthrough, Type 42 with synthesized cmaps, PS
//! Type 0 composites for CFF/CID fonts, and vectorized fallback (Layer A
//! Tasks 5+7, Layer B Tasks 2+5).
//!
//! Glyphs reach the device one at a time; text leaves as text whenever the PDF
//! embeds a font program PostScript can execute natively:
//!
//! - **Type 1** (`/FontFile`): the font program's bytes land in the prolog — since
//!   Layer B subsetted to the used charstrings (`type1_subset`, which re-emits a
//!   fully-used program byte-identically) and deduplicated by FontFile bytes: the
//!   first dict or slice to carry a program embeds it, every later one `findfont`s
//!   it — then a copy is defined under our own name with the PDF's `/FontBBox` and
//!   a re-built `/Encoding` that maps our sequential character codes to the
//!   program's glyph names (which the subset keeps, by construction: the kept set
//!   is exactly the union of what the slices show, plus `.notdef`).
//! - **TrueType** (`/FontFile2` under a simple font) leaves as **Type 42**: one
//!   `%%BeginResource` dictionary per 256-code slice whose `/sfnts` carry the
//!   program rewritten by `sfnt::synthesize_cmap` — a (3,0) format-4 subtable
//!   installed over the slice's assigned codes, mupdf's construction, the
//!   original subtables kept. The dictionary's shape is the one gs's own
//!   `ps2write` emits, verified end-to-end (ink, txtwrite extraction and raster
//!   parity) against **gs 10.08.0 (build 2026-09-08)**, this repo's oracle:
//!   `/FontMatrix [1 0 0 1 0 0]` — the identity, because this build scales
//!   Type 42 outlines by `1/unitsPerEm` itself and a `[1/upem]` matrix would
//!   divide twice (the Layer A-era `[1/upem]` emission rendered microscopic,
//!   which read as "codes draw nothing"); `/Encoding` and `/CharStrings`
//!   carry the program's real glyph names (`post` table via skrifa, `/gNNNN`
//!   fallback), because this build resolves a shown code through
//!   Encoding → name → CharStrings → gid — the PLRM route — and its txtwrite
//!   maps those names back to Unicode through the Adobe Glyph List, which is
//!   what makes the drawn text extractable. A glyph with no real name falls
//!   back to `/gNNNN` (renders identically; extraction for that glyph
//!   degrades honestly), and a name the font duplicates across gids goes to
//!   its lowest gid, so the emission is byte-reproducible. The definition
//!   ends `currentdict end definefont pop` with exactly the name and the
//!   dict as `definefont`'s two operands (the earlier `dup begin …
//!   currentdict end definefont` left three operands, so the dict became the
//!   FontDirectory key — invisible to `findfont`, Courier substitution: the
//!   Task 2 "gs finding", an emission bug, not an environmental one). The synthesized
//!   cmap stays in the sfnts for consumers that resolve Type 42 codes through
//!   the cmap per the Type 42 convention; under this gs it is inert but
//!   harmless (the same bytes render and extract with it present). gs still
//!   *requires* a `/CharStrings` for `definefont` to accept the dict at all
//!   (verified), so the emitter always carries one. The device's `1000
//!   scalefont` convention is unchanged: identity matrix × 1000 gives exactly
//!   1 pt per 1000-unit glyph-space unit, as with Type 1.
//! - **CFF and CID fonts** (a simple font's `/FontFile3 /Type1C`, or a Type0
//!   over a CIDFontType0/CIDFontType2 descendant) leave as **Type 0
//!   composites** (Task 5): a CMap resource mapping our assigned 2-byte
//!   codes to the font's CIDs, a CIDFont resource embedding the program, and
//!   a `composefont` parent — the shapes probed executable against gs 10.08
//!   (`cid.rs` module docs carry the map). A simple CFF is *promoted* with
//!   CID == charstring gid and `Adobe`/`Identity`/0; a native CID-keyed CFF
//!   inverts its charset via skrifa (`cff_gid_to_cid`, the same parse hayro
//!   performs) so the codes are the font's own CIDs, degrading to vectorized
//!   paths with a note when the charset cannot be inverted; a CIDFontType2
//!   embeds the TrueType program verbatim with an identity CIDMap (the PDF's
//!   glyph ids survive as CIDs — hayro already applied any `/CIDToGIDMap`, and
//!   the composite's own CID→glyph chain is ours). Extraction for the CID
//!   arms resolves through the CMap→CID machinery, not glyph names, so no
//!   txtwrite name round-trip exists for them — structural emission plus ink
//!   is what is verified (the plan scopes extraction proof to the Latin
//!   arms).
//! - **Everything else** — OpenType-wrapped programs (`/FontFile3
//!   /OpenType`), Type 3, non-embedded and unreadable font programs — is
//!   vectorized through the Task 3 path machinery (`outline()` → path ops
//!   under the glyph transform), with one fidelity note per font recording
//!   why. Type 3 glyphs are the exception: their PDF drawing instructions
//!   are re-interpreted through the device itself, which is faithful.
//!
//! Geometry: the interpreter hands each glyph `transform * glyph_transform`, the
//! total map from 1000-unit glyph space to the page (findings (b)). Consecutive
//! glyphs of one font whose linear part is bit-identical (true within a text run:
//! text-matrix advances are pure translations, which compose exactly) batch into
//! a single `xyshow`: the CTM becomes the run's linear part, the font is scaled
//! so its effective glyph matrix is the identity on 1000-unit space (`1000
//! scalefont` over the program's natural matrix — 1/1000 for Type 1; identity
//! for Type 42 and the composites, where gs applies the program's own scaling),
//! and every glyph origin comes from the observed transforms, so positioning
//! never depends on the PDF's declared widths agreeing with the font's.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use hayro::hayro_interpret::CacheKey;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, Name, Number, Object, Stream};
use hayro::hayro_interpret::hayro_syntax::page::Resources;
use skrifa::GlyphId;
use skrifa::raw::TableProvider as _;
use skrifa::raw::ps::cff::CffFontRef;

use crate::cid;
use crate::psnum;
use crate::sfnt;
use crate::type1_subset::subset_type1;

/// How glyphs of one font are emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextStrategy {
    /// Embedded Type 1: program bytes in the prolog, re-encoded copy, `show` text.
    Type1Passthrough,
    /// Embedded TrueType: Type 42 dictionaries in the prolog, their sfnts
    /// rewritten with a synthesized (3,0) cmap over the assigned codes, `show`
    /// text.
    Type42Cmap,
    /// Embedded CFF (promoted simple or native CID-keyed): a Type 0 composite
    /// whose CMap maps our 2-byte codes to the font's CIDs, `show` text.
    CidFontType0,
    /// A Type0/CIDFontType2 with an embedded TrueType program: a Type 0
    /// composite over a type-11 CIDFont, `show` text.
    CidFontType2,
    /// Outline glyphs as plain path operations.
    Vectorize,
}

/// Which font program (if any) the PDF embedded for one font dict.
#[derive(Clone, Debug)]
enum EmbeddedKind {
    /// `/FontFile` — a Type 1 font program, decoded.
    Type1(Vec<u8>),
    /// `/FontFile2` — TrueType data, decoded and held for the Type 42 emission
    /// (the preamble rewrites it with a synthesized cmap per slice).
    TrueType(Vec<u8>),
    /// `/FontFile3` — a bare CFF program, decoded. `cid_keyed` is the stream
    /// dict's `/Subtype`: `CIDFontType0C` carries a charset of CIDs (the
    /// native composite arm inverts it), anything else is treated as a
    /// name-keyed `Type1C` (parse-checked before promotion).
    Cff { bytes: Vec<u8>, cid_keyed: bool },
    /// `/FontFile3 /Subtype /OpenType` — an sfnt wrapper we do not unwrap
    /// (out of scope, Layer B); the strategy vectorizes with a note.
    OpenType,
    /// No embedded program.
    None,
}

/// The memoized decision for one font, with whatever the emission needs.
enum Decision {
    /// The parsed Type 1 program: its own name (for `findfont`) and gid → glyph name.
    Type1 { name: String, names: HashMap<u32, String> },
    /// Embedded TrueType: the program lives in the entry's `kind`; the preamble
    /// synthesizes a cmap over each slice's assigned codes.
    Type42,
    /// A CFF composite. `cids` is the gid → CID table of a CID-keyed
    /// program's charset (indexed by gid); `None` means identity — either a
    /// promoted name-keyed CFF (gs renumbers CID == charstring index) or a
    /// name-keyed program under a Type0 (hayro maps gid == CID through the
    /// identity `/CIDToGIDMap`).
    CidFontType0 { cids: Option<Vec<u32>> },
    /// A Type0/CIDFontType2 composite over an embedded TrueType program:
    /// the PDF's glyph ids ride as CIDs (identity CIDMap), so no table is
    /// needed.
    CidFontType2,
    /// Path fallback; the note (if any) was queued when this was decided.
    Vectorize,
}

/// The PostScript names of one composite font's three resources, minted from
/// the same counter the simple-font slices use (so no two resources of the
/// export ever share a name).
#[derive(Clone, Debug)]
struct CompositeNames {
    /// The Type 0 parent (`PdfCraftF<n>`), the name `findfont` selects.
    font: String,
    /// The CMap resource (`PdfCraftM<n>`).
    cmap: String,
    /// The CIDFont descendant (`PdfCraftD<n>`).
    descendant: String,
}

/// One pseudo-font of at most 256 assigned character codes. A font whose text
/// needs more than 256 distinct glyphs spills into further slices, each with its
/// own `/Encoding` and PostScript font name.
struct Slice {
    /// The PostScript font name (`PdfCraftF<n>`), unique per slice.
    name: String,
    /// glyph id → assigned code in this slice.
    codes: HashMap<u32, u8>,
}

/// One font from the page's resource dictionaries.
struct FontEntry {
    kind: EmbeddedKind,
    /// `/Subtype` of the font dict, when readable.
    subtype: Option<String>,
    /// For a Type0 font: the descendant's `/Subtype` (CIDFontType0/2), when
    /// readable — read from `/DescendantFonts[0]`, everything copied out.
    descendant_subtype: Option<String>,
    /// `/BaseFont` of the font dict, when readable (names the family in notes).
    base_font: Option<String>,
    /// `/FontDescriptor /FontBBox` (or the font dict's own, for Type 3).
    bbox: Option<[f64; 4]>,
    /// The descendant's `/CIDSystemInfo`, when readable (Type0 fonts only;
    /// consumed by the CIDFontType 2 emission — the ReadData form of the
    /// CIDFontType 0 has no home for it).
    csi: Option<cid::CidSystemInfo>,
    /// The descendant's `/W`, re-serialized as a PostScript array of numbers
    /// and nested arrays (anything else in the PDF array → `None` → `/DW
    /// 1000`). Carried by the CIDFontType 2 emission; never positions
    /// anything under this device (`xyshow` supplies every displacement).
    w: Option<String>,
    decision: Option<Decision>,
    slices: Vec<Slice>,
    /// gid → assigned composite code (code = CID; gid 0 remapped to the
    /// highest unused code). Filled by `cid_code` as glyphs arrive.
    cid_codes: HashMap<u32, u16>,
    /// The composite's resource names, minted at the first `cid_code`.
    composite: Option<CompositeNames>,
}

impl FontEntry {
    /// The entry for a font the resource walk never saw (nested resources, or a
    /// fallback font with no dict of its own).
    fn unknown() -> Self {
        Self {
            kind: EmbeddedKind::None,
            subtype: None,
            descendant_subtype: None,
            base_font: None,
            bbox: None,
            csi: None,
            w: None,
            decision: None,
            slices: Vec::new(),
            cid_codes: HashMap::new(),
            composite: None,
        }
    }
}

/// Per-font memoization for the text strategies (Layer A, Task 5).
///
/// Built once from the page's font resources (including the parent chain)
/// before interpretation; glyph `font_cache_key()`s — the font dict cache keys,
/// computed by the same `CacheKey` impl the interpreter uses — index it, so the
/// key reported by a glyph finds the same dict the interpreter resolved.
#[derive(Default)]
pub(crate) struct FontTable {
    fonts: HashMap<u128, FontEntry>,
    /// First-use order, for a deterministic preamble.
    order: Vec<u128>,
    /// Fidelity notes queued by strategy decisions, drained by the device.
    pending_notes: Vec<String>,
    /// Global counter handing out PostScript font names, one per slice.
    slice_counter: usize,
    /// SHA-256 of an emitted Type 1 FontFile stream → the PostScript name its
    /// program filed itself under. The first dict carrying a program records
    /// it here; a later dict whose FontFile bytes are identical `findfont`s
    /// that name instead of embedding the bytes again (byte-identical
    /// programs carry the same `/FontName` by construction, and the emitted
    /// subset covers every sharing dict's glyphs — see [`FontTable::preamble`]).
    programs: HashMap<[u8; 32], String>,
}

impl FontTable {
    /// Merge one page's font resources (own and inherited) into the table and
    /// extract, per font dict, the embedded program bytes, subtype and bounding
    /// box. Everything is copied, so the table outlives the PDF objects. Font
    /// dicts already known (the same indirect object seen from another page's
    /// resources) keep their entry, so decisions and slice numbering carry
    /// across the pages of one export and never produce two PostScript fonts
    /// for one PDF font.
    pub(crate) fn add_resources(&mut self, resources: &Resources<'_>) {
        let mut level = Some(resources.clone());
        while let Some(res) = level {
            for key in res.fonts.keys() {
                let Some(dict) = res.fonts.get::<Dict<'_>>(key.as_ref()) else {
                    continue;
                };
                let cache_key = CacheKey::cache_key(&dict);
                if self.fonts.contains_key(&cache_key) {
                    continue;
                }
                self.order.push(cache_key);
                self.fonts.insert(cache_key, entry_of(&dict));
            }
            level = res.parent().cloned();
        }
    }

    /// Decide (once per font) how its glyphs are emitted. Queues a fidelity note
    /// when the decision degrades.
    pub(crate) fn strategy(&mut self, glyph: &Glyph<'_>) -> TextStrategy {
        let Glyph::Outline(outline) = glyph else {
            // Type 3 glyphs have no font program; the device re-interprets them.
            return TextStrategy::Vectorize;
        };
        let key = outline.font_cache_key();
        if !self.fonts.contains_key(&key) {
            self.order.push(key);
            self.fonts.insert(key, FontEntry::unknown());
        }
        // The entry exists (inserted above or by the resource walk).
        if let Some(entry) = self.fonts.get(&key)
            && entry.decision.is_some()
        {
            return Self::strategy_of(entry);
        }
        let entry = self.fonts.get(&key);
        let family = || entry.and_then(|e| e.base_font.as_deref()).unwrap_or("unnamed");
        let decision = match (entry.map(|e| &e.kind), entry.and_then(|e| e.subtype.as_deref())) {
            // A Type0 font: everything composite reads from the descendant
            // (walked at entry time). The glyph's cache key indexes the
            // parent dict, which is the dict the interpreter resolved.
            (_, Some("Type0")) => match entry.map(|e| &e.kind) {
                Some(EmbeddedKind::Cff { bytes, cid_keyed: true }) => {
                    // A native CID-keyed CFF: the codes must be the font's
                    // own CIDs, the inverse of its charset (the same parse
                    // hayro performs, so the gids agree by construction).
                    match cff_gid_to_cid(bytes) {
                        Some(cids) => Decision::CidFontType0 { cids: Some(cids) },
                        None => {
                            self.pending_notes.push(format!("text vectorized (CFF '{}', charset unreadable)", family()));
                            Decision::Vectorize
                        }
                    }
                }
                Some(EmbeddedKind::Cff { bytes, cid_keyed: false }) => {
                    // A name-keyed program under a Type0: hayro maps gid ==
                    // CID through the identity /CIDToGIDMap, so identity
                    // codes carry it (parse-checked first).
                    if cff_parses(bytes) {
                        Decision::CidFontType0 { cids: None }
                    } else {
                        self.pending_notes.push(format!("text vectorized (CFF '{}' program unreadable)", family()));
                        Decision::Vectorize
                    }
                }
                Some(EmbeddedKind::TrueType(program)) => {
                    // The program ships verbatim inside the type-11 CIDFont,
                    // and `cidfont2_ready` dress-rehearses the exact parse
                    // the preamble performs (`head` upem plus the `maxp`
                    // that sizes the CIDMap), holding the type42_ready
                    // invariant: never a page referencing a font that never
                    // got defined.
                    match entry.and_then(|e| e.descendant_subtype.as_deref()) {
                        Some("CIDFontType2") if cidfont2_ready(program) => Decision::CidFontType2,
                        Some("CIDFontType2") => {
                            self.pending_notes.push(format!("text vectorized (CID TrueType '{}' program unreadable)", family()));
                            Decision::Vectorize
                        }
                        _ => {
                            self.pending_notes.push("text vectorized (CID font program)".into());
                            Decision::Vectorize
                        }
                    }
                }
                _ => {
                    self.pending_notes.push("text vectorized (CID font program)".into());
                    Decision::Vectorize
                }
            },
            (Some(EmbeddedKind::Type1(program)), st) if st != Some("TrueType") => match parse_type1(program) {
                Some((name, names)) if !names.is_empty() => Decision::Type1 { name, names },
                _ => {
                    self.pending_notes.push("text vectorized (font program unreadable)".into());
                    Decision::Vectorize
                }
            },
            (Some(EmbeddedKind::Type1(_)), _) => {
                self.pending_notes.push("text vectorized (font program unreadable)".into());
                Decision::Vectorize
            }
            (Some(EmbeddedKind::TrueType(program)), _) => {
                // The Type 42 arm (Layer B): the preamble rewrites the program
                // with a (3,0) cmap over the assigned codes — the only route by
                // which our codes draw under gs (module docs). The decision
                // dress-rehearsals that rewrite, so a program that cannot carry
                // it vectorizes here, before any code is assigned.
                if type42_ready(program) {
                    Decision::Type42
                } else {
                    self.pending_notes.push(format!("text vectorized (Type 42 '{}' program unreadable)", family()));
                    Decision::Vectorize
                }
            }
            (Some(EmbeddedKind::Cff { bytes, cid_keyed: false }), _) => {
                // The promotion arm: a simple font's CFF becomes a composite
                // with CID == charstring gid. Parse-checked so gs's loader
                // never meets bytes skrifa already rejected.
                if cff_parses(bytes) {
                    Decision::CidFontType0 { cids: None }
                } else {
                    self.pending_notes.push(format!("text vectorized (CFF '{}' program unreadable)", family()));
                    Decision::Vectorize
                }
            }
            (Some(EmbeddedKind::Cff { cid_keyed: true, .. }), _) => {
                // A CID-keyed program under a simple font is malformed (its
                // CIDs belong to a character collection a simple font cannot
                // name); decline honestly.
                self.pending_notes.push(format!("text vectorized (CID-keyed CFF '{}' under a simple font)", family()));
                Decision::Vectorize
            }
            (Some(EmbeddedKind::OpenType), _) => {
                self.pending_notes.push(format!("text vectorized (OpenType-wrapped CFF '{}')", family()));
                Decision::Vectorize
            }
            _ => {
                self.pending_notes.push("text vectorized (font not embedded)".into());
                Decision::Vectorize
            }
        };
        if let Some(entry) = self.fonts.get_mut(&key) {
            entry.decision = Some(decision);
        }
        self.fonts.get(&key).map(Self::strategy_of).unwrap_or(TextStrategy::Vectorize)
    }

    fn strategy_of(entry: &FontEntry) -> TextStrategy {
        match entry.decision {
            Some(Decision::Type1 { .. }) => TextStrategy::Type1Passthrough,
            Some(Decision::Type42) => TextStrategy::Type42Cmap,
            Some(Decision::CidFontType0 { .. }) => TextStrategy::CidFontType0,
            Some(Decision::CidFontType2) => TextStrategy::CidFontType2,
            _ => TextStrategy::Vectorize,
        }
    }

    /// The sequential character code for `glyph_id`, assigning the next free code
    /// of the font's current slice; a full slice spills to a new pseudo-font.
    /// Codes start at 1: Ghostscript renders character code 0 as nothing
    /// regardless of the Encoding (verified), so it stays reserved for
    /// `.notdef` and each slice holds codes 1..=255.
    pub(crate) fn char_code(&mut self, font_key: u128, glyph_id: u32) -> u8 {
        if !self.fonts.contains_key(&font_key) {
            self.order.push(font_key);
            self.fonts.insert(font_key, FontEntry::unknown());
        }
        let counter = &mut self.slice_counter;
        if let Some(entry) = self.fonts.get_mut(&font_key) {
            // A glyph already seen keeps the code its slice assigned it, even
            // after later glyphs spilled into further slices.
            if let Some(&code) = entry.slices.iter().find_map(|s| s.codes.get(&glyph_id)) {
                return code;
            }
            let needs_slice = entry.slices.last().is_none_or(|s| s.codes.len() >= 255);
            if needs_slice {
                let name = format!("PdfCraftF{counter}");
                *counter += 1;
                entry.slices.push(Slice { name, codes: HashMap::new() });
            }
            // `needs_slice` guarantees a slice with room below 255, so the code
            // always fits a byte.
            if let Some(slice) = entry.slices.last_mut()
                && slice.codes.len() < 255
            {
                let code = slice.codes.len() as u8 + 1;
                slice.codes.insert(glyph_id, code);
                return code;
            }
        }
        0
    }

    /// The index of the slice holding `glyph_id` (assignments are append-only,
    /// so it is the newest slice that contains it).
    pub(crate) fn slice_index(&self, font_key: u128, glyph_id: u32) -> Option<usize> {
        self.fonts.get(&font_key)?.slices.iter().rposition(|s| s.codes.contains_key(&glyph_id))
    }

    /// The PostScript font names of a font's slices, in order.
    pub(crate) fn slice_names(&self, font_key: u128) -> Vec<String> {
        self.fonts.get(&font_key).map_or_else(Vec::new, |e| e.slices.iter().map(|s| s.name.clone()).collect())
    }

    /// The glyph name a Type 1 program gives a glyph id (its `/Encoding` entry).
    pub(crate) fn type1_glyph_name(&self, font_key: u128, glyph_id: u32) -> Option<&str> {
        let names = match self.fonts.get(&font_key)?.decision.as_ref() {
            Some(Decision::Type1 { names, .. }) => names,
            _ => return None,
        };
        names.get(&glyph_id).map(String::as_str)
    }

    /// The composite character code for `glyph_id`: **code = CID** — the
    /// font's own CID for a native CID-keyed CFF (the decision's charset
    /// inverse), the gid itself for identity fonts (promotion, CIDFontType2).
    /// gid 0, whose codes the 1-byte arms must keep away from `.notdef`,
    /// remaps to the **highest unused code** (`0xFFFF` downward, skipping any
    /// code a sibling glyph's CID already took), and every assignment is
    /// memoized so repeated glyphs keep their codes. The caller guarantees
    /// `glyph_id <= 0xFFFE`; CIDs themselves are charset values and never
    /// exceed 16 bits.
    pub(crate) fn cid_code(&mut self, font_key: u128, glyph_id: u32) -> u16 {
        if !self.fonts.contains_key(&font_key) {
            self.order.push(font_key);
            self.fonts.insert(font_key, FontEntry::unknown());
        }
        let counter = &mut self.slice_counter;
        if let Some(entry) = self.fonts.get_mut(&font_key) {
            if let Some(&code) = entry.cid_codes.get(&glyph_id) {
                return code;
            }
            if entry.composite.is_none() {
                let n = *counter;
                *counter += 1;
                entry.composite =
                    Some(CompositeNames { font: format!("PdfCraftF{n}"), cmap: format!("PdfCraftM{n}"), descendant: format!("PdfCraftD{n}") });
            }
            let cid = Self::cid_of(entry, glyph_id);
            let code = if glyph_id == 0 {
                // The highest unused code, walking down past any CID a
                // sibling already claimed; 0xFFFF is free unless some glyph's
                // CID is exactly it, and the loop cannot run off the bottom
                // without first finding a free code (there are at most 65536
                // assigned codes).
                let mut candidate = u16::MAX;
                while entry.cid_codes.values().any(|&used| used == candidate) {
                    candidate -= 1;
                }
                candidate
            } else {
                // Charset values are u16 and identity gids are ≤ 0xFFFE, so
                // the conversion cannot truncate on the guarded path.
                u16::try_from(cid).unwrap_or(0xFFFF)
            };
            entry.cid_codes.insert(glyph_id, code);
            return code;
        }
        0
    }

    /// The CID a composite font's glyph `gid` draws: the charset inverse's
    /// entry when the decision carries one, the gid itself otherwise
    /// (identity fonts). Falls back to the gid for a gid past the table —
    /// unreachable through the strategy, which sized the table from the same
    /// parse the glyph ids come from.
    fn cid_of(entry: &FontEntry, gid: u32) -> u32 {
        match entry.decision.as_ref() {
            Some(Decision::CidFontType0 { cids: Some(cids) }) => cids.get(gid as usize).copied().unwrap_or(gid),
            _ => gid,
        }
    }

    /// The PostScript font name of a font's composite (`PdfCraftF<n>`), for
    /// `findfont` at flush time. `None` until the first `cid_code` minted it.
    pub(crate) fn cid_name(&self, font_key: u128) -> Option<String> {
        self.fonts.get(&font_key)?.composite.as_ref().map(|c| c.font.clone())
    }

    /// Take the notes queued since the last drain.
    pub(crate) fn drain_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notes)
    }

    /// The font definitions for the prolog (`%%BeginResource` per PostScript
    /// font, in first-use order). Vectorized fonts define nothing.
    ///
    /// Type 1 programs are subsetted to the union of the glyph names the
    /// export shows ([`type1_subset::subset_type1`]) — a union, because font
    /// dicts whose FontFile bytes are byte-identical share **one** program
    /// blob: the first occurrence embeds the subset, later ones (other dicts,
    /// or later slices of the same font) only `findfont` it under the name it
    /// defined itself. A program that cannot be subset embeds verbatim (Layer
    /// A behaviour, only larger, no note); `programs` is the dedup ledger,
    /// keyed by SHA-256 of the FontFile bytes.
    pub(crate) fn preamble(&mut self) -> Vec<u8> {
        // Pass 1 — the union of used glyph names per program bytes. Every
        // dict sharing a program shares the one emitted subset, so it must
        // cover them all: two dicts with identical bytes may well show
        // disjoint glyph sets.
        let mut used: HashMap<[u8; 32], std::collections::BTreeSet<String>> = HashMap::new();
        for i in 0..self.order.len() {
            let Some(key) = self.order.get(i).copied() else { continue };
            let Some(entry) = self.fonts.get(&key) else { continue };
            if let (Some(Decision::Type1 { names, .. }), EmbeddedKind::Type1(program)) = (entry.decision.as_ref(), &entry.kind) {
                let shown = used.entry(program_hash(program)).or_default();
                for slice in &entry.slices {
                    for gid in slice.codes.keys() {
                        if let Some(name) = names.get(gid) {
                            shown.insert(name.clone());
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for i in 0..self.order.len() {
            let Some(key) = self.order.get(i).copied() else { continue };
            let Some(entry) = self.fonts.get(&key) else { continue };
            match entry.decision.as_ref() {
                Some(Decision::Type1 { name, names }) => {
                    let EmbeddedKind::Type1(program) = &entry.kind else { continue };
                    // Nothing shown means nothing to define — and no claim on
                    // the program, which a later dict sharing these bytes may
                    // still need embedded (mirrors the composite arm's empty-
                    // codes guard).
                    if entry.slices.is_empty() {
                        continue;
                    }
                    let hash = program_hash(program);
                    // The first slice of the first dict carrying these bytes
                    // embeds the subset; every later slice — this dict's own
                    // or another dict's — `findfont`s it (`take()` hands the
                    // bytes to exactly one resource).
                    let mut program = match self.programs.entry(hash) {
                        Entry::Occupied(_) => None, // already in the prolog; the copies findfont it
                        Entry::Vacant(vacant) => {
                            vacant.insert(name.clone());
                            // The subset covers the union of every sharing
                            // dict's names; a declined subset embeds the
                            // program verbatim (correct output, only larger).
                            let wanted: Vec<&str> = used.get(&hash).map(|set| set.iter().map(String::as_str).collect()).unwrap_or_default();
                            Some(subset_type1(program, &wanted).unwrap_or_else(|| program.to_vec()))
                        }
                    };
                    // The findfont target: the name the emitted program
                    // filed itself under (byte-identical bytes carry the
                    // same /FontName, so this is `name` itself — the map
                    // makes the sharing explicit).
                    let target = self.programs.get(&hash).cloned().unwrap_or_else(|| name.clone());
                    for slice in &entry.slices {
                        type1_resource(&mut out, slice, &target, names, program.take().as_deref(), entry.bbox);
                    }
                }
                Some(Decision::Type42) => {
                    let EmbeddedKind::TrueType(program) = &entry.kind else { continue };
                    // Each slice synthesizes its own cmap over the codes it
                    // owns; slices beyond the first duplicate the sfnts
                    // (acceptable — W3 shrinks them).
                    for slice in &entry.slices {
                        type42_resource(&mut out, slice, program, entry.bbox);
                    }
                }
                Some(Decision::CidFontType0 { cids }) => {
                    let EmbeddedKind::Cff { bytes, .. } = &entry.kind else { continue };
                    let mappings = composite_mappings(entry, cids.as_deref());
                    // gs's own CFF loader route: the CFF promotes to the
                    // CIDFont resource, carrying its own metrics and
                    // CIDSystemInfo (the near-passthrough decision).
                    composite_preamble(&mut out, entry, &mappings, |out, names| cid::cidfont_type0(out, &names.descendant, bytes));
                }
                Some(Decision::CidFontType2) => {
                    let EmbeddedKind::TrueType(program) = &entry.kind else { continue };
                    // These parses mirror `cidfont2_ready`, the strategy-time
                    // gate, so a font that reaches this decision always
                    // carries them; the skips stay as guards, never as the
                    // only line of defense (a `continue` here would emit a
                    // page that references a font that never got defined).
                    let Some(font) = skrifa::FontRef::from_index(program, 0).ok() else { continue };
                    let Ok(maxp) = font.maxp() else { continue };
                    let mappings = composite_mappings(entry, None);
                    let csi = entry.csi.clone().unwrap_or_else(cid::identity_csi);
                    // The type-11 key set around the PDF-transliterated ones;
                    // the CIDMap is sized by the program's glyph count (the
                    // strategy's parse gate guaranteed it reads).
                    composite_preamble(&mut out, entry, &mappings, |out, names| {
                        cid::cidfont_type2(out, &names.descendant, program, maxp.num_glyphs(), &csi, entry.w.as_deref());
                    });
                }
                _ => {}
            }
        }
        out
    }
}

/// The CMap mappings of a composite font's assigned codes: `(code, cid)` for
/// every gid `cid_code` has seen, the code the device shows and the CID that
/// reaches the glyph (identity fonts: the gid; native fonts: the charset
/// inverse's entry). The CMap emitter sorts and collapses them.
fn composite_mappings(entry: &FontEntry, cids: Option<&[u32]>) -> Vec<(u16, u32)> {
    entry
        .cid_codes
        .iter()
        .map(|(gid, code)| (*code, cids.and_then(|c| c.get(usize::try_from(*gid).ok().unwrap_or(usize::MAX))).copied().unwrap_or(*gid)))
        .collect()
}

/// One composite font's preamble block — CMap, descendant, parent, in the
/// design's order — under their `%%BeginResource` comments. Nothing is
/// emitted for a composite no glyph reached (no codes, no CMap to build);
/// `descendant` receives the names and writes the CIDFont resource.
fn composite_preamble(out: &mut Vec<u8>, entry: &FontEntry, mappings: &[(u16, u32)], descendant: impl FnOnce(&mut Vec<u8>, &CompositeNames)) {
    let Some(names) = entry.composite.as_ref() else { return };
    if entry.cid_codes.is_empty() {
        return;
    }
    cid::cmap_resource(out, &names.cmap, 2, mappings);
    descendant(out, names);
    cid::type0_parent(out, &names.font, &names.cmap, &names.descendant);
}

/// One re-encoded copy of a Type 1 program. `program` is `Some(bytes)` for
/// the resource that embeds them — the first dict or slice carrying those
/// FontFile bytes, subsetted to the export's used glyph names — and `None`
/// for every later one, which `findfont`s the program under its own name
/// instead of embedding it again (`original` is that name either way).
fn type1_resource(out: &mut Vec<u8>, slice: &Slice, original: &str, names: &HashMap<u32, String>, program: Option<&[u8]>, bbox: Option<[f64; 4]>) {
    out.extend_from_slice(format!("%%BeginResource: font {}\n", slice.name).as_bytes());
    if let Some(program) = program {
        emit_program_bytes(out, program);
    }
    out.extend_from_slice(format!("/{n} /{original} findfont dup length dict copy\n", n = slice.name).as_bytes());
    // A built font carries an FID entry; definefont rejects a copy that keeps it.
    out.extend_from_slice(b"dup /FID known { dup /FID undef } if\n");
    if let Some(bbox) = bbox {
        out.extend_from_slice(format!("dup /FontBBox {} put\n", bbox_text(bbox)).as_bytes());
    }
    out.extend_from_slice(format!("dup /FontName /{} put\n", slice.name).as_bytes());
    out.extend_from_slice(b"dup /Encoding [");
    encoding_names(out, &slice.codes.iter().map(|(gid, code)| (*code, gid_name(names, *gid))).collect::<Vec<_>>());
    out.extend_from_slice(b"] put\ndefinefont pop\n%%EndResource\n");
}

/// One Type 42 pseudo-font over an embedded TrueType program: the sfnt
/// rewritten by [`sfnt::subset_glyphs`] (kept: the slice's glyphs and their
/// composite components, ids stable, unused glyphs emptied) and then
/// [`sfnt::synthesize_cmap`] (a (3,0) format-4 subtable over the slice's
/// assigned codes, the original subtables kept) — a subset the surgery
/// cannot carry, or one whose cmap synthesis declines, falls back to the
/// whole program (output identical, only larger) — wrapped in a
/// `/FontType 42` dictionary in the shape gs's own
/// `ps2write` emits — the shape verified end-to-end against gs 10.08.0
/// (build 2026-09-08), this repo's oracle:
///
/// - `/FontMatrix [1 0 0 1 0 0]` — the identity, *not* `1/unitsPerEm`: this
///   gs build scales Type 42 glyph outlines by `1/upem` itself before the
///   font matrix applies, so a `[1/upem]` matrix divides twice and renders
///   the text microscopic (measured: a 36 pt em at 0.018 pt). The device's
///   `1000 scalefont` convention then lands exactly 1 pt per 1000-unit
///   glyph-space unit, unchanged from the Type 1 arm.
/// - `/Encoding` and `/CharStrings` carry the program's *real* glyph names
///   (its `post` table, CFF charset as skrifa resolves them): this build
///   resolves a `show`n code through `/Encoding` → name → `/CharStrings` →
///   gid — the PLRM route, *not* the sfnt cmap (proven: a font whose cmap
///   maps no assigned code still draws through the names) — and gs's
///   txtwrite maps those names back to Unicode through the Adobe Glyph
///   List, which is what makes the drawn text extractable. The tables are
///   built from the ORIGINAL program's gids, and that stays correct over
///   the subset precisely because the surgery keeps ids stable. A glyph
///   with no real name falls back to `/gNNNN` (renders identically;
///   extraction for that glyph degrades honestly). The synthesized cmap
///   stays in the sfnts for consumers that follow the Type 42 convention
///   of resolving codes through the cmap; it is harmless under this gs
///   (the same bytes render and extract with it present).
/// - `/PaintType 0` and `/CharStrings` (`.notdef` → 0, one name per assigned
///   gid) are gs's `definefont` requirements for a Type 42 dict (verified;
///   ps2write emits both).
/// - The definition is `/Name N dict begin … currentdict end definefont
///   pop` — the name and the dict are exactly `definefont`'s two operands.
///   (The previous `dup begin … currentdict end definefont` shape left
///   three operands on the stack, so the *dict* became the FontDirectory
///   key — filed as `--nostringval--`, invisible to `findfont`, Courier
///   substitution: the whole Task 2 "gs finding".)
///
/// `/FontBBox` comes from the PDF when it carries one. The sfnts are chunked
/// per the Type 42 convention — at most 65535 bytes per string, every chunk
/// but the last a multiple of four. A program the decision somehow cannot
/// rewrite defines nothing rather than shipping a broken dictionary — the
/// strategy arm dress-rehearsed the same rewrite, so this is not a route
/// input can reach.
fn type42_resource(out: &mut Vec<u8>, slice: &Slice, program: &[u8], bbox: Option<[f64; 4]>) {
    // gs divides Type 42 outlines by `head.unitsPerEm` itself, so a zero
    // upem breaks the consumer even though the emitted matrix is identity.
    if units_per_em(program).is_none() {
        return;
    }
    let mappings: Vec<(u16, u16)> = slice
        .codes
        .iter()
        // A gid past the u16 the cmap carries cannot resolve and is dropped,
        // exactly as `synthesize_cmap` drops it below `numGlyphs`.
        .filter_map(|(gid, code)| Some((u16::from(*code), u16::try_from(*gid).ok()?)))
        .collect();
    // The one-pass pipeline: subset the program to the slice's glyph closure
    // (Task 7 — stable gids, unused glyphs emptied, components followed),
    // then synthesize the cmap over the subset — each step that declines
    // falls back to the whole program before giving up: a subset the surgery
    // cannot carry embeds it verbatim, and a subset whose cmap synthesis
    // declines is retried over the whole program (stable gids keep the same
    // mappings correct). Every fallback emits what the un-subsetted pipeline
    // would, only larger, so no note (silence-on-correct); the `return` needs
    // the whole-program synthesis to fail too, which `type42_ready` rehearsed
    // at strategy time — the page's `findfont` cannot miss.
    let used: Vec<u32> = slice.codes.keys().copied().collect();
    let subset = sfnt::subset_glyphs(program, &used).unwrap_or_else(|| program.to_vec());
    let Some(sfnts) = sfnt::synthesize_cmap(&subset, &mappings).or_else(|| sfnt::synthesize_cmap(program, &mappings)) else { return };
    // The load-bearing names: real when the program carries them, `/gNNNN`
    // otherwise, unique within the slice, never `.notdef` or an unusable
    // PostScript literal.
    let font = match skrifa::FontRef::from_index(program, 0) {
        Ok(font) => font,
        Err(_) => return,
    };
    let names = skrifa::GlyphNames::new(&font);
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let name_of = |gid: u32, used: &mut std::collections::HashSet<String>| match names.get(skrifa::GlyphId::new(gid)) {
        Some(name) if !name.is_synthesized() && usable_ps_name(name.as_str()) => {
            let name = name.as_str().to_string();
            if name == ".notdef" || !used.insert(name.clone()) { glyph_fallback_name(gid) } else { name }
        }
        _ => glyph_fallback_name(gid),
    };
    // Duplicate real names resolve deterministically: the pairs are sorted by
    // gid before names are assigned, so when the font's `post` table gives two
    // used gids the same name (legal in post 2.0) the lower gid keeps it and
    // the higher falls back — `HashMap` iteration order is random per process
    // and must not pick the winner, or the same input PDF would export to
    // different bytes across runs.
    let mut pairs: Vec<(u32, u8)> = slice.codes.iter().map(|(gid, code)| (*gid, *code)).collect();
    pairs.sort_unstable_by_key(|&(gid, _)| gid);
    let assigned: Vec<(u32, u8, String)> = pairs.into_iter().map(|(gid, code)| (gid, code, name_of(gid, &mut used))).collect();
    out.extend_from_slice(format!("%%BeginResource: font {}\n", slice.name).as_bytes());
    out.extend_from_slice(format!("/{} 11 dict begin\n", slice.name).as_bytes());
    out.extend_from_slice(format!("/FontName /{} def\n", slice.name).as_bytes());
    out.extend_from_slice(b"/FontType 42 def\n");
    out.extend_from_slice(b"/PaintType 0 def\n");
    out.extend_from_slice(b"/FontMatrix [1 0 0 1 0 0] def\n");
    if let Some(bbox) = bbox {
        out.extend_from_slice(format!("/FontBBox {} def\n", bbox_text(bbox)).as_bytes());
    }
    out.extend_from_slice(b"/Encoding [");
    encoding_names(out, &assigned.iter().map(|(_, code, name)| (*code, name.clone())).collect::<Vec<_>>());
    out.extend_from_slice(b"] def\n");
    // The gs-required name → gid table, over the same names the Encoding
    // carries (`assigned` is already in gid order).
    out.extend_from_slice(b"/CharStrings 256 dict dup begin\n/.notdef 0 def\n");
    for (gid, _, name) in &assigned {
        out.extend_from_slice(format!("/{name} {gid} def\n").as_bytes());
    }
    out.extend_from_slice(b"end def\n");
    out.extend_from_slice(b"/sfnts [\n");
    const CHUNK: usize = 65520;
    for chunk in sfnts.chunks(CHUNK) {
        out.extend_from_slice(hex_payload(chunk).as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"] def\ncurrentdict end definefont pop\n%%EndResource\n");
}

/// The fallback glyph name for `gid` when the program carries no real one:
/// the `/gNNNN` convention (unique per gid by construction, a valid literal
/// name, and meaningless to the Adobe Glyph List — so extraction degrades
/// for that glyph while rendering is unaffected).
fn glyph_fallback_name(gid: u32) -> String {
    format!("g{gid:04}")
}

/// Whether `name` can ride into the emission as a PostScript literal name:
/// non-empty, short, and free of the delimiters and whitespace the tokenizer
/// would break on (real-world `post` names are, but a hostile font's are not
/// bound by that).
fn usable_ps_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 127
        && name.bytes().all(|b| matches!(b, b'!'..=b'~') && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'))
}

/// `head.unitsPerEm` of an sfnt, when the program parses and the value is
/// usable: gs divides Type 42 glyph outlines by the upem itself, so a zero
/// upem breaks the consumer's scaling even though the emitted `/FontMatrix`
/// is the identity.
fn units_per_em(program: &[u8]) -> Option<u16> {
    let font = skrifa::FontRef::from_index(program, 0).ok()?;
    let upem = font.head().ok()?.units_per_em();
    (upem != 0).then_some(upem)
}

/// Whether the Type 42 emission can carry `program`: skrifa parses it
/// (`FontRef` + `head` with a usable upem) and [`sfnt::synthesize_cmap`]
/// succeeds over it. The synthesis call is the strategy-time dress rehearsal
/// of the rewrite the preamble performs per slice — the same pipeline with an
/// empty mapping set, which cannot overflow where a full one could — so a
/// font whose tables or old-cmap extents defeat the rewrite vectorizes before
/// any code is assigned, instead of emitting a page that references a font
/// that never got defined.
fn type42_ready(program: &[u8]) -> bool {
    units_per_em(program).is_some() && sfnt::synthesize_cmap(program, &[]).is_some()
}

/// Whether the CIDFontType 2 emission can carry `program`: the same parse the
/// preamble performs — `FontRef` + `head` with a usable upem *and* a readable
/// `maxp`, whose glyph count sizes the `/CIDMap` and `/CIDCount`. This is the
/// strategy-time dress rehearsal of that emission, holding the `type42_ready`
/// invariant: a program the preamble cannot carry vectorizes here, before any
/// code is assigned, instead of emitting a page that references a font that
/// never got defined.
fn cidfont2_ready(program: &[u8]) -> bool {
    let Ok(font) = skrifa::FontRef::from_index(program, 0) else { return false };
    font.head().is_ok_and(|head| head.units_per_em() != 0) && font.maxp().is_ok()
}

/// A PostScript hex string literal carrying `bytes`: pure-ASCII lowercase hex
/// inside `<…>`, wrapped near 200 columns — whitespace is free inside a hex
/// string, so the wrap cannot change the bytes a consumer reads. Task 4's
/// CID-keyed emission reuses this for its `/sfnts` too.
pub(crate) fn hex_payload(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2).saturating_add(2));
    out.push('<');
    let mut column = 1usize;
    for byte in bytes {
        if column + 2 > 200 {
            out.push('\n');
            column = 0;
        }
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
        column += 2;
    }
    out.push('>');
    out
}

/// Build the walk result for one font dict. For a Type0 the descendant dict
/// (`/DescendantFonts[0]`) supplies the subtype, the descriptor program,
/// `/W` and `/CIDSystemInfo`; everything is copied out, as the table must
/// outlive the PDF objects.
fn entry_of(dict: &Dict<'_>) -> FontEntry {
    // A Type0's program and metrics live on the descendant; every other
    // subtype keeps its own dict as the source.
    let is_type0 = dict.get::<Name<'_>>(b"Subtype").map(|n| n.as_str() == "Type0").unwrap_or(false);
    let descendant = if is_type0 { dict.get::<Array<'_>>(b"DescendantFonts").and_then(|fonts| fonts.iter::<Dict<'_>>().next()) } else { None };
    let source = descendant.as_ref().unwrap_or(dict);
    let kind = embedded_kind(source);
    FontEntry {
        kind,
        subtype: dict.get::<Name<'_>>(b"Subtype").map(|n| n.as_str().to_string()),
        descendant_subtype: descendant.as_ref().and_then(|d| d.get::<Name<'_>>(b"Subtype")).map(|n| n.as_str().to_string()),
        base_font: dict.get::<Name<'_>>(b"BaseFont").map(|n| n.as_str().to_string()),
        bbox: bbox_of(dict).or_else(|| descendant.as_ref().and_then(bbox_of)),
        csi: source.get::<Dict<'_>>(b"CIDSystemInfo").and_then(|d| csi_of(&d)),
        w: source.get::<Array<'_>>(b"W").and_then(|a| w_array(&a)),
        decision: None,
        slices: Vec::new(),
        cid_codes: HashMap::new(),
        composite: None,
    }
}

/// The embedded font program of a font dict's descriptor: `/FontFile` (Type
/// 1), `/FontFile2` (TrueType) or `/FontFile3` (a bare CFF whose stream
/// `/Subtype` picks the flavor: `CIDFontType0C` is CID-keyed, `OpenType` is
/// the wrapper we decline, anything else — `Type1C`, or a missing subtype —
/// is treated as a name-keyed CFF the parse gate checks). Decoding validates
/// the program (an undecodable one falls through to the not-embedded note).
fn embedded_kind(dict: &Dict<'_>) -> EmbeddedKind {
    let Some(desc) = dict.get::<Dict<'_>>(b"FontDescriptor") else {
        return EmbeddedKind::None;
    };
    if let Some(stream) = desc.get::<Stream<'_>>(b"FontFile") {
        return stream.decoded().ok().map(|d| EmbeddedKind::Type1(d.into_owned())).unwrap_or(EmbeddedKind::None);
    }
    if let Some(stream) = desc.get::<Stream<'_>>(b"FontFile2") {
        // The bytes are held for the Type 42 / CIDFontType 2 emission.
        return stream.decoded().ok().map(|d| EmbeddedKind::TrueType(d.into_owned())).unwrap_or(EmbeddedKind::None);
    }
    if let Some(stream) = desc.get::<Stream<'_>>(b"FontFile3") {
        let subtype = stream.dict().get::<Name<'_>>(b"Subtype").map(|n| n.as_str().to_string());
        return match subtype.as_deref() {
            Some("OpenType") => EmbeddedKind::OpenType,
            Some("CIDFontType0C") => {
                stream.decoded().ok().map(|d| EmbeddedKind::Cff { bytes: d.into_owned(), cid_keyed: true }).unwrap_or(EmbeddedKind::None)
            }
            // Type1C — or a missing/unknown subtype, which the CFF parse
            // gate arbitrates before anything is emitted.
            _ => stream.decoded().ok().map(|d| EmbeddedKind::Cff { bytes: d.into_owned(), cid_keyed: false }).unwrap_or(EmbeddedKind::None),
        };
    }
    EmbeddedKind::None
}

/// A `/CIDSystemInfo` dict copied out: Registry and Ordering as strings (a
/// Name in the wild is accepted too — both are ASCII identifiers), Supplement
/// as an integer.
fn csi_of(dict: &Dict<'_>) -> Option<cid::CidSystemInfo> {
    let read = |key: &[u8]| -> Option<String> {
        match dict.get::<Object<'_>>(key) {
            Some(Object::String(s)) => Some(String::from_utf8_lossy(&s).into_owned()),
            Some(Object::Name(n)) => Some(n.as_str().to_string()),
            _ => None,
        }
    };
    Some(cid::CidSystemInfo {
        registry: read(b"Registry")?,
        ordering: read(b"Ordering")?,
        supplement: i32::try_from(dict.get::<Number>(b"Supplement")?.as_i64()).ok()?,
    })
}

/// A PDF `/W` array re-serialized as a PostScript array: numbers and nested
/// arrays of numbers only (the forms the PDF spec allows — `c [w]`,
/// `c [w1 w2]` and `c c w`), brackets included, numbers through the operand
/// formatter. Anything else — a Name, String, Dict, non-finite number, deeper
/// nesting, or **an empty array at either level** (a level with no objects
/// has no `]` to balance a `[` for, and `/W ]` is a gs error that kills the
/// whole program) — makes the whole array uncarriable → `None` → `/DW 1000`.
fn w_array(array: &Array<'_>) -> Option<String> {
    fn emit_objects<'a>(objects: impl Iterator<Item = Object<'a>>, depth: u8) -> Option<String> {
        let mut out = String::from("[");
        let mut any = false;
        for obj in objects {
            match obj {
                Object::Number(n) => {
                    out.push_str(&psnum::num(n.as_f64())?);
                    out.push(' ');
                    any = true;
                }
                Object::Array(inner) if depth == 0 => {
                    out.push_str(&emit_objects(inner.iter::<Object<'_>>(), 1)?);
                    out.push(' ');
                    any = true;
                }
                _ => return None,
            }
        }
        if !any {
            return None;
        }
        // Only the trailing space of the last object; `any` guarantees one is there.
        out.pop();
        out.push(']');
        Some(out)
    }
    emit_objects(array.iter::<Object<'_>>(), 0)
}

/// Whether a bare CFF program parses through skrifa — the same parse hayro
/// performs, and the dress rehearsal for gs's ReadData loader before a simple
/// font's CFF is promoted.
fn cff_parses(cff: &[u8]) -> bool {
    CffFontRef::new(cff, 0, None).is_ok()
}

/// The gid → CID table of a CID-keyed CFF: the inverse view of its charset,
/// read through skrifa's `CffFontRef` (the same parse hayro performs, so the
/// glyph ids agree by construction — hayro hands the device the charstring
/// index and keeps the CID→glyph mapping inside this same charset).
/// `None` when the program is not CID-keyed, or its charset cannot be read
/// for every glyph (truncated data, claimed glyph count past the charset,
/// more glyphs than a 2-byte CID can address) — the caller degrades with a
/// note rather than emitting codes that would draw wrong glyphs.
fn cff_gid_to_cid(cff: &[u8]) -> Option<Vec<u32>> {
    let font = CffFontRef::new(cff, 0, None).ok()?;
    if !font.is_cid() {
        return None;
    }
    let charset = font.charset()?;
    let num_glyphs = font.num_glyphs();
    // Charstring INDEX counts are 16-bit; a font claiming more glyphs than a
    // 2-byte CID addresses cannot ride the composite route at all.
    if num_glyphs > 0xFFFF {
        return None;
    }
    let mut cids = Vec::with_capacity(num_glyphs as usize);
    for gid in 0..num_glyphs {
        // Charset values are u16 SIDs-or-CIDs; a glyph the charset does not
        // cover would draw as .notdef under the PDF's own CID, so the whole
        // table declines.
        cids.push(u32::from(charset.string_id(GlyphId::new(gid))?.to_u16()));
    }
    Some(cids)
}

/// `/FontBBox` from the descriptor, falling back to the font dict itself (Type 3).
fn bbox_of(dict: &Dict<'_>) -> Option<[f64; 4]> {
    let source = dict.get::<Dict<'_>>(b"FontDescriptor");
    let array = source.as_ref().and_then(|d| d.get::<Array<'_>>(b"FontBBox")).or_else(|| dict.get::<Array<'_>>(b"FontBBox"))?;
    let mut bbox = [0.0; 4];
    for (slot, obj) in bbox.iter_mut().zip(array.iter::<Object<'_>>()) {
        let Object::Number(n) = obj else {
            return None;
        };
        *slot = n.as_f64();
    }
    bbox.iter().all(|v| v.is_finite()).then_some(bbox)
}

/// The SHA-256 of a Type 1 FontFile stream — the prolog's dedup key for
/// already-embedded programs (a real digest, not a hand-rolled one: a
/// collision here would emit a page that references a font whose glyphs were
/// subsetted away). The same helper `xfa` uses to find pictures again.
fn program_hash(program: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(program).into()
}

/// Parse a Type 1 font program into its own name and its glyph-name table — the
/// same parse hayro performs (same skrifa), so the glyph ids agree by construction.
fn parse_type1(program: &[u8]) -> Option<(String, HashMap<u32, String>)> {
    let font = skrifa::raw::ps::type1::Type1Font::new(program).ok()?;
    let name = font.name()?.to_string();
    let names = font.glyph_names().map(|(gid, name)| (gid.to_u32(), name.to_string())).collect();
    Some((name, names))
}

/// A PostScript font bbox `[x0 y0 x1 y1]` (values already validated finite).
fn bbox_text(bbox: [f64; 4]) -> String {
    let nums: Vec<String> = bbox.iter().map(|v| psnum::num(*v).unwrap_or_else(|| "0".into())).collect();
    format!("[{}]", nums.join(" "))
}

/// The `/Encoding` entries: 256 names, assigned codes mapped to their glyphs,
/// `.notdef` elsewhere, wrapped to keep lines DSC-friendly.
fn encoding_names(out: &mut Vec<u8>, assigned: &[(u8, String)]) {
    let mut entries = vec![".notdef"; 256];
    for (code, name) in assigned {
        if let Some(slot) = entries.get_mut(usize::from(*code)) {
            *slot = name.as_str();
        }
    }
    out.push(b'\n');
    let mut line = String::new();
    for name in entries {
        let entry = format!("/{name}");
        if line.len() + entry.len() + 1 > 200 {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&entry);
    }
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
}

/// A glyph's Type 1 name as an Encoding entry (unknown ids stay `.notdef`).
fn gid_name(names: &HashMap<u32, String>, gid: u32) -> String {
    names.get(&gid).cloned().unwrap_or_else(|| ".notdef".into())
}

/// Emit a font program verbatim, bracketed as a binary DSC section when its
/// bytes are not printable ASCII (a hex-eexec PFA-style program needs nothing).
/// The `%%BeginData` count covers the payload plus its terminating newline, in the
/// DSC 3.0 form `<numberof> <type> <bytesorlines>` ("Binary Bytes" — the examples
/// in Adobe 5001.DSC_Spec.pdf, e.g. `%%BeginData: 135174 Hex Bytes`).
fn emit_program_bytes(out: &mut Vec<u8>, program: &[u8]) {
    let ascii = program.iter().all(|b| matches!(*b, b'\n' | b'\r' | b'\t' | 0x20..=0x7e));
    if ascii {
        out.extend_from_slice(program);
        out.push(b'\n');
    } else {
        out.extend_from_slice(format!("%%BeginData: {} Binary Bytes\n", program.len() + 1).as_bytes());
        out.extend_from_slice(program);
        out.push(b'\n');
        out.extend_from_slice(b"%%EndData\n");
    }
}

/// A PostScript string literal carrying `bytes` exactly: printable ASCII passes
/// through (brackets and backslash escaped), everything else — including CR and
/// LF, so no transport can reinterpret line endings inside the string — becomes
/// a 3-digit octal escape. The result is pure ASCII.
pub(crate) fn ps_string(bytes: &[u8]) -> String {
    let mut s = String::from("(");
    for &b in bytes {
        match b {
            b'(' => s.push_str("\\("),
            b')' => s.push_str("\\)"),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\{b:03o}")),
        }
    }
    s.push(')');
    s
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
    use kurbo::{Affine, Rect};

    use super::*;
    use crate::device::PsDevice;

    // ---------------------------------------------------------------- fixtures

    /// A `<< … >> stream … endstream` object body (repo fixture style; the
    /// length is computed, never counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a one-page PDF from object bodies numbered from 1, computing
    /// the xref like `crates/engine/src/tests.rs` (lengths and offsets, never
    /// counted by hand).
    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// Assemble a one-page PDF: catalog, pages, font (`/F1` = object 3),
    /// content (object 4), descriptor (5), a font-program stream (6, whatever
    /// kind the test means it to be) and optional extra streams (7…), page last.
    fn font_pdf(font: &str, descriptor: &str, fontfile_dict: &str, fontfile: &[u8], content: &[u8]) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            // The page is the last object; its number fills /Kids below.
            b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            font.as_bytes().to_vec(),
            stream_obj("", content),
            descriptor.as_bytes().to_vec(),
            stream_obj(fontfile_dict, fontfile),
        ];
        objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec());
        assemble(objs)
    }

    /// Assemble a one-page PDF with a Type0 `/F1`: the parent (object 3,
    /// `/Encoding /Identity-H` over one descendant), content (4), the
    /// descendant's descriptor (5), a font-program stream (6) and the
    /// descendant dict (7), page last (8).
    fn type0_pdf(base_font: &str, descendant: &str, descriptor: &str, fontfile_dict: &str, fontfile: &[u8], content: &[u8]) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [8 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            format!("<< /Type /Font /Subtype /Type0 /BaseFont /{base_font} /Encoding /Identity-H /DescendantFonts [7 0 R] >>").into_bytes(),
            stream_obj("", content),
            descriptor.as_bytes().to_vec(),
            stream_obj(fontfile_dict, fontfile),
            descendant.as_bytes().to_vec(),
        ];
        objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec());
        assemble(objs)
    }

    /// Adobe Type 1 encryption (eexec seed 55665, charstring seed 4330).
    fn t1_encrypt(seed: u32, data: &[u8]) -> Vec<u8> {
        let mut seed = seed;
        data.iter()
            .map(|&plain| {
                let cipher = plain ^ ((seed >> 8) as u8);
                seed = (u32::from(cipher).wrapping_add(seed)).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
                cipher
            })
            .collect()
    }

    /// A charstring: the plaintext ops, charstring-encrypted. The fixture
    /// declares `/lenIV 0`, so there is no padding prefix (Ghostscript 10.08's
    /// FreeType path fails to render otherwise-valid glyphs whose lenIV-4
    /// padding bytes it does not expect — verified empirically; lenIV 0 is a
    /// documented value).
    fn t1_charstring(ops: &[u8]) -> Vec<u8> {
        t1_encrypt(4330, ops)
    }

    /// A minimal, real Type 1 font program (PFA style, hex eexec): glyphs
    /// `.notdef`, `A` and `B`, each a 300×400 box outline in a 1000-unit em.
    /// Verified to load and self-define under `/PdfCraftTest` in Ghostscript
    /// 10.08 — the tail (`2 index /CharStrings …`, `readonly put`/`noaccess
    /// put`, `dup /FontName get exch definefont pop`) follows the structure of
    /// real fonts, which the operand stack arithmetic requires.
    fn t1_program() -> Vec<u8> {
        // Type 1 charstrings. `hsbw` takes two operands (sidebearing, width);
        // the box is `0 500 hsbw; 100 hmoveto; 0 400 rlineto; 300 0 rlineto;
        // 0 -400 rlineto; closepath; endchar` (widths as 2-byte encodings).
        const BOX_A: [u8; 20] = [139, 248, 136, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14];
        // Same outline behind a 700-unit advance.
        const BOX_B: [u8; 20] = [139, 249, 80, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14];
        const NOTDEF: [u8; 5] = [139, 248, 136, 13, 14];
        let mut private: Vec<u8> = Vec::new();
        private.extend_from_slice(
            b"dup /Private 8 dict dup begin\n\
              /RD {string currentfile exch readstring pop} executeonly def\n\
              /ND {noaccess def} executeonly def\n\
              /NP {noaccess put} executeonly def\n\
              /lenIV 0 def\n\
              2 index /CharStrings 3 dict dup begin\n",
        );
        for (name, ops) in [(".notdef", NOTDEF.as_slice()), ("A", BOX_A.as_slice()), ("B", BOX_B.as_slice())] {
            let cipher = t1_charstring(ops);
            private.extend_from_slice(format!("/{name} {} RD ", cipher.len()).as_bytes());
            private.extend_from_slice(&cipher);
            // `ND` must follow the data immediately: Ghostscript's resync after
            // the charstring `readstring` mis-reads even one space (verified
            // against real font bytes with and without the separator).
            private.extend_from_slice(b"ND\n");
        }
        private.extend_from_slice(
            b"end\nend\nreadonly put\nnoaccess put\n\
              dup /FontName get exch definefont pop\n\
              mark currentfile closefile\n",
        );
        let mut plain = vec![0x5a, 0xa5, 0x37, 0xc3];
        plain.extend_from_slice(&private);
        let cipher = t1_encrypt(55665, &plain);
        #[rustfmt::skip]
        let cleartext: &str = "%!PS-AdobeFont-1.0: PdfCraftTest 001.001\n\
             11 dict begin\n\
             /FontName /PdfCraftTest def\n\
             /PaintType 0 def\n\
             /FontType 1 def\n\
             /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n\
             /Encoding StandardEncoding def\n\
             /FontBBox [0 -200 700 800] readonly def\n\
             currentdict end\n\
             currentfile eexec\n";
        let mut out = cleartext.as_bytes().to_vec();
        // One unbroken hex line, like pfbtopfa output: Ghostscript's resync
        // after each charstring `readstring` mis-handles newlines inside the
        // hex (verified: the same bytes with wrapped lines fail to load).
        out.extend_from_slice(cipher.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes());
        out.push(b'\n');
        // The classic trailer: eight lines of 64 zeros and cleartomark.
        for _ in 0..8 {
            out.extend_from_slice(&[b'0'; 64]);
            out.push(b'\n');
        }
        out.extend_from_slice(b"cleartomark\n");
        out
    }

    /// [`t1_program`] with 'A' composing through `seac`: `0 500 hsbw
    /// 50 0 66 66 endchar` — four operands into a final `endchar`, the
    /// implicit spelling of Type 1's standard-encoding accent construction
    /// (skrifa's evaluator — the parse both hayro and the oracle ride —
    /// resolves operands 66/66 as base/accent codes 'B'/'B' through
    /// StandardEncoding). 'B' stays a plain box, so the composing glyph
    /// renders from real charstrings during the drive.
    fn seac_t1_program() -> Vec<u8> {
        // `0 500 hsbw 50 0 66 66 endchar`: adx=50, ady=0, bchar=achar='B'.
        const SEAC_A: [u8; 9] = [139, 248, 136, 13, 189, 139, 205, 205, 14];
        const BOX_B: [u8; 20] = [139, 249, 80, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14];
        const NOTDEF: [u8; 5] = [139, 248, 136, 13, 14];
        let mut private: Vec<u8> = Vec::new();
        private.extend_from_slice(
            b"dup /Private 8 dict dup begin\n\
              /RD {string currentfile exch readstring pop} executeonly def\n\
              /ND {noaccess def} executeonly def\n\
              /NP {noaccess put} executeonly def\n\
              /lenIV 0 def\n\
              2 index /CharStrings 3 dict dup begin\n",
        );
        for (name, ops) in [(".notdef", NOTDEF.as_slice()), ("A", SEAC_A.as_slice()), ("B", BOX_B.as_slice())] {
            let cipher = t1_charstring(ops);
            private.extend_from_slice(format!("/{name} {} RD ", cipher.len()).as_bytes());
            private.extend_from_slice(&cipher);
            private.extend_from_slice(b"ND\n");
        }
        private.extend_from_slice(
            b"end\nend\nreadonly put\nnoaccess put\n\
              dup /FontName get exch definefont pop\n\
              mark currentfile closefile\n",
        );
        let mut plain = vec![0x5a, 0xa5, 0x37, 0xc3];
        plain.extend_from_slice(&private);
        let cipher = t1_encrypt(55665, &plain);
        let mut out = b"%!PS-AdobeFont-1.0: PdfCraftTest 001.001\n11 dict begin\n/FontName /PdfCraftTest def\n/PaintType 0 def\n/FontType 1 def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\n/FontBBox [0 -200 700 800] readonly def\ncurrentdict end\ncurrentfile eexec\n".to_vec();
        out.extend_from_slice(cipher.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes());
        out.push(b'\n');
        for _ in 0..8 {
            out.extend_from_slice(&[b'0'; 64]);
            out.push(b'\n');
        }
        out.extend_from_slice(b"cleartomark\n");
        out
    }

    /// A minimal TrueType font: 1000 upem, three glyphs (empty outlines — these
    /// fixtures test text emission, not outlines), a (1,0) format-6 cmap mapping
    /// A→1 and B→2, and a version-3.0 `post` (no names, so `/glyphNNNN`
    /// addressing is the only route — exactly what the emitter uses).
    fn ttf_program() -> Vec<u8> {
        ttf_program_with_glyphs(3)
    }

    /// [`ttf_program`] with a custom `maxp` glyph count: the slice-spilling
    /// test assigns codes for 300 gids, and `synthesize_cmap` bounds mappings
    /// against `numGlyphs`, so the font must claim them (the `glyf` data still
    /// holds two boxes — these fixtures exercise tables and text emission,
    /// not outlines).
    fn ttf_program_with_glyphs(num_glyphs: u16) -> Vec<u8> {
        let mut post = Vec::new();
        post.extend_from_slice(&[0x00, 0x03, 0x00, 0x00]); // version 3.0
        post.extend_from_slice(&[0; 4]); // italicAngle
        post.extend_from_slice(&(-100i16).to_be_bytes()); // underlinePosition
        post.extend_from_slice(&50u16.to_be_bytes()); // underlineThickness
        post.extend_from_slice(&[0; 20]); // isFixedPitch + memory limits
        ttf_program_with_post(num_glyphs, post)
    }

    /// [`ttf_program_with_glyphs`] with a caller-supplied `post` table: the
    /// duplicate-name regression installs a version-2.0 post whose gids 1 and
    /// 2 both point at one pool name.
    fn ttf_program_with_post(num_glyphs: u16, post: Vec<u8>) -> Vec<u8> {
        ttf_program_over(&[box_glyph(), box_glyph()], num_glyphs, &[1, 2], &post)
    }

    /// The fixture core: `glyphs` in gid order after an empty .notdef, a
    /// caller-chosen `maxp` count (the spilling fixture claims more than it
    /// carries), and a (1,0) format-6 cmap mapping 65.. to `mapped` (the
    /// glyph array, one gid per code from 'A').
    fn ttf_program_over(glyphs: &[Vec<u8>], num_glyphs: u16, mapped: &[u16], post: &[u8]) -> Vec<u8> {
        fn be16(v: u16) -> [u8; 2] {
            v.to_be_bytes()
        }
        fn be32(v: u32) -> [u8; 4] {
            v.to_be_bytes()
        }
        let mut head = Vec::new();
        head.extend_from_slice(&be32(0x0001_0000)); // version
        head.extend_from_slice(&be32(0x0001_0000)); // fontRevision
        head.extend_from_slice(&be32(0)); // checkSumAdjustment
        head.extend_from_slice(&be32(0x5F0F_3CF5)); // magicNumber
        head.extend_from_slice(&be16(0x0003)); // flags
        head.extend_from_slice(&be16(1000)); // unitsPerEm
        head.extend_from_slice(&[0; 8]); // created
        head.extend_from_slice(&[0; 8]); // modified
        head.extend_from_slice(&be16(0)); // xMin
        head.extend_from_slice(&be16(0xFF38)); // yMin (-200)
        head.extend_from_slice(&be16(700)); // xMax
        head.extend_from_slice(&be16(800)); // yMax
        head.extend_from_slice(&be16(0)); // macStyle
        head.extend_from_slice(&be16(8)); // lowestRecPPEM
        head.extend_from_slice(&be16(2)); // fontDirectionHint
        head.extend_from_slice(&be16(0)); // indexToLocFormat (short)
        head.extend_from_slice(&be16(0)); // glyphDataFormat
        let mut hhea = Vec::new();
        hhea.extend_from_slice(&be32(0x0001_0000));
        for v in [800i16, -200, 0, 700, 0, 0, 700, 1, 0, 0] {
            hhea.extend_from_slice(&be16(v as u16));
        }
        hhea.extend_from_slice(&[0; 8]); // reserved
        hhea.extend_from_slice(&be16(0)); // metricDataFormat
        hhea.extend_from_slice(&be16(1)); // numberOfHMetrics
        let hmtx = [be16(600), be16(0), be16(0), be16(0)].concat();
        let mut maxp = Vec::new();
        maxp.extend_from_slice(&be32(0x0001_0000));
        maxp.extend_from_slice(&be16(num_glyphs)); // numGlyphs
        maxp.extend_from_slice(&[0; 26]);
        let mut cmap = Vec::new();
        cmap.extend_from_slice(&be16(0)); // version
        cmap.extend_from_slice(&be16(1)); // numTables
        cmap.extend_from_slice(&be16(1)); // platformID: Macintosh
        cmap.extend_from_slice(&be16(0)); // encodingID: Roman
        cmap.extend_from_slice(&be32(12)); // subtable offset
        cmap.extend_from_slice(&be16(6)); // format 6
        cmap.extend_from_slice(&be16(10 + u16::try_from(mapped.len() * 2).expect("the fixture cmap fits"))); // length
        cmap.extend_from_slice(&be16(0)); // language
        cmap.extend_from_slice(&be16(65)); // firstCode ('A')
        cmap.extend_from_slice(&be16(u16::try_from(mapped.len()).expect("the fixture cmap fits"))); // entryCount
        for &gid in mapped {
            cmap.extend_from_slice(&be16(gid));
        }
        // The glyphs in gid order after the empty .notdef; short `loca` holds
        // byte offsets ÷ 2, computed from the cumulative even sizes.
        let mut glyf = Vec::new();
        let mut loca: Vec<u8> = Vec::new();
        loca.extend_from_slice(&be16(0)); // glyph 0 empty at 0
        loca.extend_from_slice(&be16(0));
        for glyph in glyphs {
            glyf.extend_from_slice(glyph);
            let half = u16::try_from(glyf.len() / 2).expect("the fixture glyf fits the short loca");
            loca.extend_from_slice(&be16(half));
        }

        // Directory entries, alphabetically by tag as the format requires.
        let tables: [(&[u8; 4], &[u8]); 8] = [
            (b"cmap", &cmap),
            (b"glyf", &glyf),
            (b"head", &head),
            (b"hhea", &hhea),
            (b"hmtx", &hmtx),
            (b"loca", &loca),
            (b"maxp", &maxp),
            (b"post", post),
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&be32(0x0001_0000));
        out.extend_from_slice(&be16(tables.len() as u16));
        out.extend_from_slice(&be16(16 * 8)); // searchRange
        out.extend_from_slice(&be16(3)); // entrySelector
        out.extend_from_slice(&be16(0)); // rangeShift
        let mut offset = 12 + 16 * tables.len();
        let mut dir = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            dir.extend_from_slice(tag.as_slice());
            dir.extend_from_slice(&be32(0)); // checksum (none of our readers check it)
            dir.extend_from_slice(&be32(offset as u32));
            dir.extend_from_slice(&be32(data.len() as u32));
            body.extend_from_slice(data);
            let pad = data.len().next_multiple_of(4) - data.len();
            body.extend(std::iter::repeat_n(0, pad));
            offset += data.len() + pad;
        }
        out.extend_from_slice(&dir);
        out.extend_from_slice(&body);
        out
    }

    /// A simple glyph for the fixture core: a 300×400 box at x=100, 34 bytes
    /// (even-sized, as the short `loca` format requires of every glyph).
    fn box_glyph() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(&0x0001u16.to_be_bytes()); // numberOfContours
        g.extend_from_slice(&100i16.to_be_bytes()); // xMin
        g.extend_from_slice(&0i16.to_be_bytes()); // yMin
        g.extend_from_slice(&400i16.to_be_bytes()); // xMax
        g.extend_from_slice(&400i16.to_be_bytes()); // yMax
        g.extend_from_slice(&3u16.to_be_bytes()); // endPtsOfContours[0] (4 points)
        g.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        g.extend_from_slice(&[0x01; 4]); // flags: on-curve, 16-bit deltas
        for d in [100i16, 0, 300, 0, 0, 400, 0, -400] {
            g.extend_from_slice(&d.to_be_bytes());
        }
        g
    }

    /// A composite glyph for the fixture core: the -1 contour count, a bbox,
    /// then one record per `(gid, dx)` component carrying
    /// `ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES` (and `MORE_COMPONENTS` on
    /// all but the last) — even-sized so the short `loca` stays encodable.
    fn composite_glyph(components: &[(u16, i16)]) -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(&0xFFFFu16.to_be_bytes()); // numberOfContours: -1
        g.extend_from_slice(&0i16.to_be_bytes()); // xMin
        g.extend_from_slice(&0i16.to_be_bytes()); // yMin
        g.extend_from_slice(&700i16.to_be_bytes()); // xMax
        g.extend_from_slice(&400i16.to_be_bytes()); // yMax
        for (i, &(gid, dx)) in components.iter().enumerate() {
            let flags = 0x0001u16 | 0x0002 | if i + 1 < components.len() { 0x0010 } else { 0 };
            g.extend_from_slice(&flags.to_be_bytes());
            g.extend_from_slice(&gid.to_be_bytes());
            g.extend_from_slice(&dx.to_be_bytes()); // arg1 (an x offset)
            g.extend_from_slice(&0i16.to_be_bytes()); // arg2
        }
        g
    }

    /// The composite-glyph fixture: gids 1/2 simple boxes, gid 3 a composite
    /// over them and gid 4 an unused box; the cmap maps A→1, B→2 and C→3 so
    /// content can draw a component and the composite. With `cycle`, gid 3
    /// references *itself* instead — the malformed font the whole-font
    /// fallback test embeds.
    fn composite_ttf_program(cycle: bool) -> Vec<u8> {
        let glyph3 = if cycle { composite_glyph(&[(3, 0)]) } else { composite_glyph(&[(1, 0), (2, 300)]) };
        let glyphs = vec![box_glyph(), box_glyph(), glyph3, box_glyph()];
        let mut post = Vec::new();
        post.extend_from_slice(&[0x00, 0x03, 0x00, 0x00]); // version 3.0
        post.extend_from_slice(&[0; 4]); // italicAngle
        post.extend_from_slice(&(-100i16).to_be_bytes()); // underlinePosition
        post.extend_from_slice(&50u16.to_be_bytes()); // underlineThickness
        post.extend_from_slice(&[0; 20]); // isFixedPitch + memory limits
        ttf_program_over(&glyphs, 5, &[1, 2, 3], &post)
    }

    /// A minimal CFF (Type1C) font: `.notdef`, `A` (one curve) and `B` (a box).
    fn cff_program() -> Vec<u8> {
        /// A CFF INDEX over `items` (offSize 1; the fixture stays far below 256).
        fn index(items: &[&[u8]]) -> Vec<u8> {
            let mut out = (items.len() as u16).to_be_bytes().to_vec();
            if items.is_empty() {
                return out;
            }
            out.push(1); // offSize
            let mut offsets = vec![1u16];
            let mut total = 1u16;
            let mut data = Vec::new();
            for item in items {
                data.extend_from_slice(item);
                // The fixture stays far below the u16 range; saturate rather
                // than wrap if that ever changes.
                total = total.saturating_add(item.len() as u16);
                offsets.push(total);
            }
            for off in offsets {
                out.push(off as u8);
            }
            out.extend_from_slice(&data);
            out
        }
        fn dict_int(value: u32, op: u8) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&value.to_be_bytes());
            out.push(op);
            out
        }
        let header = [0x01, 0x00, 0x04, 0x02].to_vec();
        let name_index = index(&[b"PdfCraftCFF"]);
        let empty_index = index(&[]);
        // Type 2 charstrings: .notdef = endchar; A = `500 100 hmoveto, one
        // rrcurveto, endchar`; B = `700 100 hmoveto, box, endchar`.
        let notdef = vec![14u8];
        let a = vec![248, 136, 239, 22, 189, 189, 189, 89, 139, 39, 8, 14];
        let b = vec![249, 80, 239, 22, 248, 36, 7, 247, 192, 6, 252, 36, 7, 14];
        let charstrings = index(&[&notdef, &a, &b]);
        // Charset format 0: gid 1 → SID 34 ('A'), gid 2 → SID 35 ('B').
        let charset = [0u8, 0, 34, 0, 35].to_vec();
        // Private DICT: defaultWidthX 0.
        let private = vec![139, 20];

        // Offsets for the Top DICT. Its own size is value-independent (fixed
        // 5-byte operands), so it is measured with placeholder operands first.
        let top_len = dict_int(0, 15).len() + dict_int(0, 17).len() + 5 + 5 + 1;
        let top_index_len = 2 + 1 + 2 + top_len;
        let base = header.len() + name_index.len() + top_index_len + empty_index.len() * 2;
        let charset_pos = base;
        let charstrings_pos = charset_pos + charset.len();
        let private_pos = charstrings_pos + charstrings.len();

        let mut top = Vec::new();
        top.extend(dict_int(charset_pos as u32, 15));
        top.extend(dict_int(charstrings_pos as u32, 17));
        top.extend_from_slice(&[29]);
        top.extend_from_slice(&(private.len() as u32).to_be_bytes());
        top.extend_from_slice(&[29]);
        top.extend_from_slice(&(private_pos as u32).to_be_bytes());
        top.push(18);
        let top_index = index(&[&top]);

        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(&name_index);
        out.extend_from_slice(&top_index);
        out.extend_from_slice(&empty_index); // String INDEX
        out.extend_from_slice(&empty_index); // Global Subr INDEX
        out.extend_from_slice(&charset);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&private);
        out
    }

    /// A minimal CID-keyed CFF (CIDFontType0C-shaped): ROS Adobe/Identity/0
    /// via custom SIDs 391/392, one-FD FDArray over a Private DICT, FDSelect
    /// format 0, and a format-0 charset mapping gid 1 → CID 10 and gid 2 →
    /// CID 20 — deliberately non-identity, so a test can prove the emitted
    /// codes are the font's CIDs and not the gids.
    fn cid_cff_program() -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            let mut out = (items.len() as u16).to_be_bytes().to_vec();
            if items.is_empty() {
                return out;
            }
            out.push(1); // offSize
            let mut offsets = vec![1u16];
            let mut total = 1u16;
            let mut data = Vec::new();
            for item in items {
                data.extend_from_slice(item);
                total = total.saturating_add(item.len() as u16);
                offsets.push(total);
            }
            for off in offsets {
                out.push(off as u8);
            }
            out.extend_from_slice(&data);
            out
        }
        fn i32(value: u32) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&value.to_be_bytes());
            out
        }
        let header = [0x01, 0x00, 0x04, 0x02].to_vec();
        let name_index = index(&[b"PdfCraftCID"]);
        let empty_index = index(&[]);
        let string_index = index(&[b"Adobe", b"Identity"]); // SIDs 391, 392
        let notdef = vec![14u8];
        let a = vec![248, 136, 239, 22, 189, 189, 189, 89, 139, 39, 8, 14];
        let b = vec![249, 80, 239, 22, 248, 36, 7, 247, 192, 6, 252, 36, 7, 14];
        let charstrings = index(&[&notdef, &a, &b]);
        // Charset format 0 over CIDs: gid 1 → 10, gid 2 → 20.
        let charset = [0u8, 0, 10, 0, 20].to_vec();
        // FDSelect format 0: every glyph in FD 0.
        let fdselect = [0u8, 0, 0, 0].to_vec();
        // Private DICT: defaultWidthX 0.
        let private = vec![139, 20];

        // Sizes are value-independent (fixed 5-byte operands), so offsets are
        // computed with a placeholder top dict of the same length.
        let top = |charset_pos: u32, charstrings_pos: u32, fdarray_pos: u32, fdselect_pos: u32, _private_pos: u32| {
            let mut d = Vec::new();
            d.extend(i32(391));
            d.extend(i32(392));
            d.extend(i32(0));
            d.extend_from_slice(&[12, 30]); // ROS
            d.extend(i32(2));
            d.extend_from_slice(&[12, 34]); // CIDCount
            d.extend(i32(charset_pos));
            d.push(15); // charset
            d.extend(i32(charstrings_pos));
            d.push(17); // CharStrings
            d.extend(i32(fdarray_pos));
            d.extend_from_slice(&[12, 36]); // FDArray
            d.extend(i32(fdselect_pos));
            d.extend_from_slice(&[12, 37]); // FDSelect
            d
        };
        let top_len = top(0, 0, 0, 0, 0).len();
        let top_index_len = 2 + 1 + 2 + top_len;
        let font_dict = |private_pos: u32| {
            let mut d = Vec::new();
            d.extend(i32(private.len() as u32));
            d.extend(i32(private_pos));
            d.push(18); // Private
            d
        };
        let fdarray_len = 2 + 1 + 2 + font_dict(0).len();
        let base = header.len() + name_index.len() + top_index_len + string_index.len() + empty_index.len();
        let charset_pos = base;
        let fdselect_pos = charset_pos + charset.len();
        let charstrings_pos = fdselect_pos + fdselect.len();
        let fdarray_pos = charstrings_pos + charstrings.len();
        let private_pos = fdarray_pos + fdarray_len;
        let top_index = index(&[&top(charset_pos as u32, charstrings_pos as u32, fdarray_pos as u32, fdselect_pos as u32, private_pos as u32)]);
        let fdarray = index(&[&font_dict(private_pos as u32)]);

        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(&name_index);
        out.extend_from_slice(&top_index);
        out.extend_from_slice(&string_index);
        out.extend_from_slice(&empty_index); // Global Subr INDEX
        out.extend_from_slice(&charset);
        out.extend_from_slice(&fdselect);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&fdarray);
        out.extend_from_slice(&private);
        out
    }

    /// [`cff_program`] stretched to 259 glyphs so codes past one byte exist:
    /// every glyph gets the box charstring, and the charset sends the
    /// StandardEncoding names B and C to gids 258 and 7 — so the content
    /// `BC` draws gids `0x0102` and `0x0007` and the emitted codes are
    /// exactly the big-endian pair test wants.
    fn cff_program_sparse() -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            let mut out = (items.len() as u16).to_be_bytes().to_vec();
            out.push(1); // offSize (fixture stays far below the u16 range)
            let mut offsets = vec![1u16];
            let mut total = 1u16;
            let mut data = Vec::new();
            for item in items {
                data.extend_from_slice(item);
                total = total.saturating_add(item.len() as u16);
                offsets.push(total);
            }
            for off in offsets {
                out.push(off as u8);
            }
            out.extend_from_slice(&data);
            out
        }
        fn dict_int(value: u32, op: u8) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&value.to_be_bytes());
            out.push(op);
            out
        }
        const GLYPHS: u32 = 259; // gids 0..=258
        let header = [0x01, 0x00, 0x04, 0x02].to_vec();
        let name_index = index(&[b"PdfCraftCFF"]);
        let empty_index = index(&[]);
        let notdef = vec![14u8];
        let box_glyph = vec![249, 80, 239, 22, 248, 36, 7, 247, 192, 6, 252, 36, 7, 14];
        // 258 boxes after .notdef: charstring data 258 * 14 = 3612 bytes, far
        // below the u16 INDEX offsets carry.
        let mut cs_items: Vec<Vec<u8>> = Vec::with_capacity(GLYPHS as usize);
        cs_items.push(notdef.clone());
        for _ in 1..GLYPHS {
            cs_items.push(box_glyph.clone());
        }
        let refs: Vec<&[u8]> = cs_items.iter().map(|v| v.as_slice()).collect();
        let cs_data = index(&refs);
        // Charset format 0 over gids 1..=258: mostly SID 1 (space, unused);
        // gid 7 → SID 36 (C), gid 258 → SID 35 (B).
        let mut charset = vec![0u8];
        for gid in 1..GLYPHS {
            let sid: u16 = match gid {
                7 => 36,   // C
                258 => 35, // B
                _ => 1,    // space
            };
            charset.extend_from_slice(&sid.to_be_bytes());
        }
        let private = vec![139, 20];
        let top_len = dict_int(0, 15).len() + dict_int(0, 17).len() + 5 + 5 + 1;
        let top_index_len = 2 + 1 + 2 + top_len;
        let base = header.len() + name_index.len() + top_index_len + empty_index.len() * 2;
        let charset_pos = base;
        let charstrings_pos = charset_pos + charset.len();
        let private_pos = charstrings_pos + cs_data.len();
        let mut top = Vec::new();
        top.extend(dict_int(charset_pos as u32, 15));
        top.extend(dict_int(charstrings_pos as u32, 17));
        top.extend_from_slice(&[29]);
        top.extend_from_slice(&(private.len() as u32).to_be_bytes());
        top.extend_from_slice(&[29]);
        top.extend_from_slice(&(private_pos as u32).to_be_bytes());
        top.push(18);
        let top_index = index(&[&top]);
        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(&name_index);
        out.extend_from_slice(&top_index);
        out.extend_from_slice(&empty_index);
        out.extend_from_slice(&empty_index);
        out.extend_from_slice(&charset);
        out.extend_from_slice(&cs_data);
        out.extend_from_slice(&private);
        out
    }

    // ------------------------------------------------------------------ drive

    /// Drive the device over one page of `pdf` and return (program, notes,
    /// preamble). The device is built with the page's font resources, matching
    /// how Task 7's `export()` will assemble it.
    fn driven(pdf: &[u8]) -> (String, Vec<String>, String) {
        let pdf = Pdf::new(pdf.to_vec()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 200.0, 300.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = PsDevice::from_table(None, page.resources(), FontTable::default());
        interpret_page(page, &mut state, &mut device);
        device.finish();
        (
            String::from_utf8_lossy(&device.emit.out).into_owned(),
            device.emit.notes.iter().map(|n| n.text.clone()).collect(),
            String::from_utf8_lossy(&device.fonts.preamble()).into_owned(),
        )
    }

    fn contains_bytes(haystack: &str, needle: &[u8]) -> bool {
        needle.is_empty() || haystack.as_bytes().windows(needle.len()).any(|w| w == needle)
    }

    // ------------------------------------------------------- sfnts inspection

    /// A big-endian u16 at `at`, `None` when the slice is too short.
    fn be16_at(data: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
    }

    /// A big-endian u32 at `at`, `None` when the slice is too short.
    fn be32_at(data: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
    }

    /// The decoded `/sfnts` data of a preamble, one entry per resource in
    /// order: everything hex between `/sfnts [` and the closing `]` (string
    /// chunk boundaries and wrap newlines vanish — whitespace is free inside
    /// a hex string, and only hex digits survive the filter).
    fn sfnts_blobs(ps: &str) -> Vec<Vec<u8>> {
        let mut blobs = Vec::new();
        let mut rest = ps;
        while let Some(start) = rest.find("/sfnts [") {
            rest = &rest[start + 8..];
            let Some(end) = rest.find("] def") else { break };
            let hex: String = rest[..end].chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let bytes =
                hex.as_bytes().chunks(2).map(|pair| u8::from_str_radix(&String::from_utf8_lossy(pair), 16).expect("a hex digit pair")).collect();
            blobs.push(bytes);
            rest = &rest[end..];
        }
        blobs
    }

    /// The directory entry for `tag` in a raw sfnt: (offset, length).
    fn dir_entry(font: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
        let num = usize::from(be16_at(font, 4)?);
        for i in 0..num {
            let at = 12 + i * 16;
            let record = font.get(at..at.checked_add(16)?)?;
            if record.get(..4)? == tag {
                let offset = usize::try_from(u32::from_be_bytes(record.get(8..12)?.try_into().ok()?)).ok()?;
                let length = usize::try_from(u32::from_be_bytes(record.get(12..16)?.try_into().ok()?)).ok()?;
                return Some((offset, length));
            }
        }
        None
    }

    /// Look `code` up in the (3,0) format-4 subtable of `font`'s synthesized
    /// cmap the way a consumer does: find the segment, add `idDelta`.
    fn format4_gid(font: &[u8], code: u16) -> Option<u16> {
        let (off, len) = dir_entry(font, b"cmap")?;
        let cmap = font.get(off..off.checked_add(len)?)?;
        let count = usize::from(be16_at(cmap, 2)?);
        let mut sub = None;
        for i in 0..count {
            let at = 4 + i * 8;
            if be16_at(cmap, at)? == 3 && be16_at(cmap, at.checked_add(2)?)? == 0 {
                sub = usize::try_from(be32_at(cmap, at.checked_add(4)?)?).ok();
                break;
            }
        }
        let sub = sub?;
        let seg_count = usize::from(be16_at(cmap, sub.checked_add(6)?)?) / 2;
        let end_codes = sub.checked_add(14)?;
        let start_codes = end_codes.checked_add(seg_count * 2 + 2)?;
        let id_deltas = start_codes.checked_add(seg_count * 2)?;
        for i in 0..seg_count {
            let end = be16_at(cmap, end_codes.checked_add(i * 2)?)?;
            let start = be16_at(cmap, start_codes.checked_add(i * 2)?)?;
            if (start..=end).contains(&code) {
                let delta = be16_at(cmap, id_deltas.checked_add(i * 2)?)?;
                return Some(code.wrapping_add(delta));
            }
        }
        None
    }

    const T1_FONT: &str =
        "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftTest /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>";
    const T1_DESC: &str = "<< /Type /FontDescriptor /FontName /PdfCraftTest /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 6 0 R >>";
    const TT_FONT: &str =
        "<< /Type /Font /Subtype /TrueType /BaseFont /PdfCraftTT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>";
    const TT_DESC: &str = "<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>";

    // ------------------------------------------------------------------ tests

    /// An embedded Type 1 font's program bytes land in the prolog verbatim, with
    /// the PDF's `/FontBBox` and a re-encoded copy under our own font name.
    /// (Layer B subsets the program, but this fixture draws every glyph, and a
    /// fully-used program re-emits byte-identically — so the verbatim assert
    /// stays true, now by construction.)
    #[test]
    fn type1_fontfile_bytes_land_in_prolog() {
        let program = t1_program();
        let pdf = font_pdf(T1_FONT, T1_DESC, "", &program, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "Type 1 passthrough degrades nothing");
        assert!(contains_bytes(&preamble, &program), "program bytes verbatim in the prolog");
        assert!(preamble.contains("%%BeginResource: font PdfCraftF0\n"), "our font resource: {preamble}");
        assert!(preamble.contains("/PdfCraftF0 /PdfCraftTest findfont dup length dict copy\n"), "copy under our name: {preamble}");
        assert!(preamble.contains("dup /FontBBox [0 -200 700 800] put\n"), "PDF's FontBBox applied: {preamble}");
        assert!(preamble.contains("/Encoding [\n/.notdef /A /B "), "codes 1 and 2 re-encoded to A and B: {preamble}");
        assert!(preamble.contains("definefont pop\n%%EndResource\n"), "font defined: {preamble}");
        // And the page draws text with it: one run, both codes, geometry from
        // the observed transforms (A's 600-unit advance at 24 pt → 600 in
        // 1000-unit glyph space).
        assert!(out.contains("/PdfCraftF0 findfont 1000 scalefont setfont\n"), "font selected: {out}");
        assert!(out.contains("(\\001\\002) [600 0 0 0] xyshow\n"), "batched show: {out}");
        assert_eq!(out.matches("gsave").count(), out.matches("grestore").count(), "balanced wrap: {out}");
    }

    /// Two font dicts whose FontFile streams are byte-identical (different
    /// `/BaseFont`s) share one program blob: the first dict to reach the
    /// prolog embeds the subsetted program, the second `findfont`s it — the
    /// program's cleartext header appears exactly once while both re-encoded
    /// copies exist and the page draws with both. The dicts draw *disjoint*
    /// glyphs (F1 'A', F2 'B'), pinning the subtle half: the one emitted
    /// subset covers the union of every sharing dict's names, so the second
    /// dict's copy finds its glyph in a program it did not embed. (Layer B
    /// Task 8's dedup; the table runs across the whole document, so pages
    /// hit the same map — the next test drives the per-slice half.)
    #[test]
    fn identical_programs_across_dicts_dedup() {
        let program = t1_program();
        let font2 = "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftTwin /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 8 0 R >>";
        let desc2 = "<< /Type /FontDescriptor /FontName /PdfCraftTwin /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 9 0 R >>";
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            T1_FONT.as_bytes().to_vec(),
            stream_obj("", b"BT /F1 24 Tf 20 100 Td (A) Tj ET\nBT /F2 24 Tf 20 200 Td (B) Tj ET"),
            T1_DESC.as_bytes().to_vec(),
            stream_obj("", &program),
            font2.as_bytes().to_vec(),
            desc2.as_bytes().to_vec(),
            stream_obj("", &program),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R /F2 7 0 R >> >> >>".to_vec(),
        ];
        let pdf = assemble(objs);
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the shared program degrades nothing");
        assert_eq!(preamble.matches("%!PS-AdobeFont-1.0: PdfCraftTest").count(), 1, "one program blob: {preamble}");
        assert!(
            preamble.contains("%%BeginResource: font PdfCraftF0\n") && preamble.contains("%%BeginResource: font PdfCraftF1\n"),
            "both re-encoded copies exist: {preamble}"
        );
        assert!(
            preamble.contains("/PdfCraftF0 /PdfCraftTest findfont dup length dict copy\n")
                && preamble.contains("/PdfCraftF1 /PdfCraftTest findfont dup length dict copy\n"),
            "both copies findfont the one program: {preamble}"
        );
        assert!(out.contains("/PdfCraftF0 findfont") && out.contains("/PdfCraftF1 findfont"), "the page draws with both: {out}");
        assert_eq!(out.matches(" show\n").count(), 2, "one run per font (a single glyph emits plain `show`): {out}");
        // The one subset covers both dicts' glyphs: decrypt the emitted eexec
        // (the seed advances by the ciphertext byte, so this is not
        // `t1_encrypt` run backwards) and find both charstrings.
        let eexec = preamble.find("currentfile eexec\n").expect("the eexec marker") + "currentfile eexec\n".len();
        let line_end = eexec + preamble[eexec..].find('\n').expect("the hex line ends");
        let cipher: Vec<u8> = preamble.as_bytes()[eexec..line_end]
            .chunks(2)
            .map(|pair| u8::from_str_radix(&String::from_utf8_lossy(pair), 16).expect("a hex pair"))
            .collect();
        let mut seed: u32 = 55665;
        let plain: Vec<u8> = cipher
            .iter()
            .map(|&c| {
                let plain = c ^ ((seed >> 8) as u8);
                seed = (u32::from(c).wrapping_add(seed).wrapping_mul(52845).wrapping_add(22719)) & 0xFFFF;
                plain
            })
            .collect();
        let plain = String::from_utf8_lossy(&plain);
        assert!(contains_bytes(&plain, b"/A "), "F1's glyph survives: {plain:?}");
        assert!(contains_bytes(&plain, b"/B "), "F2's glyph survives in the program it did not embed: {plain:?}");
    }

    /// A font whose text spills into a second slice embeds its program once:
    /// the second slice's resource `findfont`s the first's program (Layer B
    /// Task 8; Layer A re-embedded the whole program per slice). Driven at
    /// the `FontTable` level, like the Type 42 slice test — a fixture font
    /// with >255 real glyphs is impractical to synthesize, and the codes
    /// assign regardless of the program's glyph count.
    #[test]
    fn type1_second_slice_findfonts_the_first() {
        let program = t1_program();
        let (name, names) = parse_type1(&program).expect("the fixture parses");
        let mut table = FontTable::default();
        let key = 42u128;
        table.order.push(key);
        table.fonts.insert(
            key,
            FontEntry {
                kind: EmbeddedKind::Type1(program),
                subtype: Some("Type1".into()),
                descendant_subtype: None,
                base_font: Some("PdfCraftTest".into()),
                bbox: Some([0.0, -200.0, 700.0, 800.0]),
                csi: None,
                w: None,
                decision: Some(Decision::Type1 { name, names }),
                slices: Vec::new(),
                cid_codes: HashMap::new(),
                composite: None,
            },
        );
        for gid in 0..300u32 {
            table.char_code(key, gid);
        }
        let preamble = String::from_utf8_lossy(&table.preamble()).into_owned();
        assert_eq!(preamble.matches("%!PS-AdobeFont-1.0: PdfCraftTest").count(), 1, "one program blob across the slices: {preamble}");
        assert_eq!(preamble.matches("%%BeginResource: font").count(), 2, "one resource per slice: {preamble}");
        assert!(
            preamble.contains("/PdfCraftF1 /PdfCraftTest findfont dup length dict copy\n"),
            "the second slice findfonts the first's program: {preamble}"
        );
    }

    /// A program whose eexec section is corrupt *after* the cleartext header
    /// — the tail is cut, so the charstrings still parse (the strategy keeps
    /// the passthrough arm; no note) but the subsetter declines
    /// (`subset_type1` → `None`): emission falls back to the Layer A verbatim
    /// path, bytes and all. A deeper cut that defeats even `parse_type1`
    /// still vectorizes — the unreadable-program arm, re-proven here.
    #[test]
    fn unparseable_program_falls_back_verbatim() {
        let whole = t1_program();
        let marker = whole.windows(b"currentfile eexec\n".len()).position(|w| w == b"currentfile eexec\n").expect("the eexec marker");
        let hex_start = marker + "currentfile eexec\n".len();
        let hex_end = hex_start + whole[hex_start..].iter().position(|&b| b == b'\n').expect("the hex line ends");
        let mut corrupt = whole[..hex_end - 30].to_vec();
        corrupt.extend_from_slice(&whole[hex_end..]);
        // Unit half: the tail cut declines, and skrifa still parses — that
        // combination is exactly what keeps the emission on the passthrough
        // arm instead of vectorizing.
        assert!(crate::type1_subset::subset_type1(&corrupt, &["A"]).is_none(), "the corrupt tail declines the subset");
        assert!(parse_type1(&corrupt).is_some_and(|(name, names)| name == "PdfCraftTest" && !names.is_empty()), "skrifa still parses it");
        let pdf = font_pdf(T1_FONT, T1_DESC, "", &corrupt, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the verbatim fallback is not a degradation");
        assert!(contains_bytes(&preamble, &corrupt), "the corrupt program rides verbatim (Layer A behaviour): {preamble}");
        assert!(preamble.contains("/PdfCraftF0 /PdfCraftTest findfont dup length dict copy\n"), "the copy still findfonts it: {preamble}");
        assert!(out.contains("xyshow"), "the text still draws through the font: {out}");
        // The deeper cut: even parse_type1 fails, so the glyphs vectorize
        // (the existing unreadable-program arm, unchanged).
        let deep = &whole[..600];
        let pdf = font_pdf(T1_FONT, T1_DESC, "", deep, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("unreadable")), "noted: {notes:?}");
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text-as-text: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
    }

    /// A program whose kept glyph composes accents (`seac`) declines the
    /// subset and embeds verbatim (fix round 1): the composing charstring
    /// references its base and accent by name — here 'A' composes from 'B' —
    /// and the subset would drop 'B' when nothing else shows it, breaking
    /// 'A' in the consumer with no note. The whole program rides verbatim,
    /// the copy still findfonts it, the text still draws, nothing degrades.
    #[test]
    fn seac_program_falls_back_verbatim() {
        let program = seac_t1_program();
        // Unit half: the composing shape declines; a plain program of the
        // same construction subsets (the detector does not over-fire).
        assert!(crate::type1_subset::subset_type1(&program, &["A"]).is_none(), "the seac charstring declines the subset");
        assert!(crate::type1_subset::subset_type1(&t1_program(), &["A"]).is_some(), "the plain fixture still subsets");
        let pdf = font_pdf(T1_FONT, T1_DESC, "", &program, b"BT /F1 24 Tf 20 100 Td (A) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the verbatim fallback is not a degradation");
        assert!(contains_bytes(&preamble, &program), "the whole program rides verbatim, B included: {preamble}");
        assert!(preamble.contains("/PdfCraftF0 /PdfCraftTest findfont dup length dict copy\n"), "the copy still findfonts it: {preamble}");
        assert!(out.contains("/PdfCraftF0 findfont") && out.contains(" show\n"), "the composing glyph still draws through the font: {out}");
    }

    /// An embedded TrueType font leaves as Type 42 text: no degradation note;
    /// the preamble defines a `/FontType 42` dictionary per slice whose
    /// `/sfnts` carry the program rewritten by cmap synthesis — the (3,0)
    /// format-4 subtable maps codes 1,2 → gids 1,2 — with the identity
    /// `/FontMatrix` (gs 10.08 applies 1/upem itself) and the PDF's
    /// `/FontBBox`; and the page draws through the same run machinery as
    /// Type 1 (`1000 scalefont`, 1 pt per 1000-unit glyph space).
    #[test]
    fn embedded_truetype_emits_type42_with_synthesized_cmap() {
        let ttf = ttf_program();
        let pdf = font_pdf(TT_FONT, TT_DESC, "", &ttf, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "Type 42 emission degrades nothing");
        assert!(preamble.contains("%%BeginResource: font PdfCraftF0\n"), "our font resource: {preamble}");
        assert!(preamble.contains("/FontType 42 def\n"), "a Type 42 dictionary: {preamble}");
        assert!(
            preamble.contains("/FontMatrix [1 0 0 1 0 0] def\n"),
            "identity matrix — gs 10.08 divides Type 42 outlines by upem itself, so 1/upem would divide twice: {preamble}"
        );
        assert!(preamble.contains("/FontBBox [0 -200 700 800] def\n"), "PDF's FontBBox applied: {preamble}");
        assert!(preamble.contains("/g0001 /g0002"), "informational Encoding names the assigned gids: {preamble}");
        assert!(preamble.contains("currentdict end definefont pop\n%%EndResource\n"), "font defined: {preamble}");
        // The sfnts are exactly the program rewritten over the assigned codes
        // (codes 1,2 → gids 1,2), and their subtable resolves like a consumer's.
        let blobs = sfnts_blobs(&preamble);
        assert_eq!(blobs.len(), 1, "one slice, one sfnts string: {preamble}");
        assert_eq!(
            blobs[0],
            crate::sfnt::synthesize_cmap(&ttf, &[(1, 1), (2, 2)]).expect("synthesis succeeds"),
            "sfnts carry the synthesized-cmap rewrite"
        );
        assert_eq!(format4_gid(&blobs[0], 1), Some(1), "code 1 → gid 1");
        assert_eq!(format4_gid(&blobs[0], 2), Some(2), "code 2 → gid 2");
        assert_eq!(format4_gid(&blobs[0], 3), None, "an unassigned code stays unmapped");
        // And the page draws text with it: one run, both codes, geometry from
        // the observed transforms (A's 600-unit advance at 24 pt → 600 in
        // 1000-unit glyph space).
        assert!(out.contains("/PdfCraftF0 findfont 1000 scalefont setfont\n"), "font selected: {out}");
        assert!(out.contains("(\\001\\002) [600 0 0 0] xyshow\n"), "batched show: {out}");
        assert_eq!(out.matches("gsave").count(), out.matches("grestore").count(), "balanced wrap: {out}");
    }

    /// A FontFile2 the Type 42 arm cannot carry declines to Type 42: the note
    /// names the strategy, the family and the reason, the glyphs vectorize (or
    /// stay empty when the interpreter itself has no outline to show — the
    /// raster renderer's view), and no font is defined or selected. Two cuts:
    /// 100 bytes defeats the parse outright, while a tail cut parses (hayro
    /// still reads `glyf` and draws substitute-able outlines) but defeats the
    /// cmap rewrite — the strategy arm's dress rehearsal, not just `FontRef`.
    #[test]
    fn truetype_program_unreadable_vectorizes() {
        let ttf = ttf_program();
        // A tail cut: the directory and `glyf` parse, the truncated table
        // extent fails the rewrite, and the outlines are path operators.
        let tail = &ttf[..ttf.len() - 40];
        let pdf = font_pdf(TT_FONT, TT_DESC, "", tail, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert!(
            notes.iter().any(|n| n.contains("Type 42") && n.contains("PdfCraftTT") && n.contains("unreadable")),
            "note names family and reason: {notes:?}"
        );
        assert!(out.contains("moveto\n") && out.contains("fill\n"), "glyph outlines as path ops: {out}");
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text-as-text: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
        assert!(!preamble.contains("/FontType 42"), "no Type 42 dictionary: {preamble}");
        // A parse-level cut: the note still fires, and the export still
        // completes (the interpreter shows nothing for such a font, so there
        // is no outline to vectorize — matching the raster renderer).
        let head = &ttf[..100];
        let pdf = font_pdf(TT_FONT, TT_DESC, "", head, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert!(
            notes.iter().any(|n| n.contains("Type 42") && n.contains("PdfCraftTT") && n.contains("unreadable")),
            "note names family and reason: {notes:?}"
        );
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text-as-text: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
        assert!(!preamble.contains("/FontType 42"), "no Type 42 dictionary: {preamble}");
    }

    /// A simple font's embedded CFF (`/FontFile3 /Type1C`) promotes to a PS
    /// Type 0 composite: no degradation note; the preamble carries the CMap
    /// (begincmap, cidrange grammar), the CIDFontType 0 descendant via gs's
    /// ReadData route with the CFF hex, and the composefont parent; and the
    /// page shows 2-byte codes — code = CID = charstring gid, so gids 1/2
    /// ride as `(\000\001\000\002)`-style octal. (Reworked from the Layer A
    /// `cff_font_vectorizes` test when the strategy flipped.)
    #[test]
    fn simple_cff_promotes_to_cidfonttype0() {
        let pdf = font_pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftCFF /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R /Encoding /StandardEncoding >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftCFF /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            "/Subtype /Type1C",
            &cff_program(),
            b"BT /F1 24 Tf 20 100 Td (AB) Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "CFF promotion degrades nothing: {notes:?}");
        assert!(preamble.contains("%%BeginResource: cmap PdfCraftM0\n"), "the CMap resource: {preamble}");
        assert!(preamble.contains("begincmap\n"), "the CMap body: {preamble}");
        assert!(preamble.contains("%%BeginResource: cidfont PdfCraftD0\n"), "the descendant resource: {preamble}");
        assert!(
            preamble.contains("/PdfCraftD0 currentfile /ASCIIHexDecode filter //true //true ReadData\n"),
            "gs's own CFF loader route: {preamble}"
        );
        assert!(
            contains_bytes(&preamble, &cff_program()) || preamble.contains(&cff_program().iter().map(|b| format!("{b:02x}")).collect::<String>()),
            "the CFF bytes ride as hex: {preamble}"
        );
        assert!(preamble.contains("/PdfCraftF0 /PdfCraftM0 [/PdfCraftD0] composefont pop\n"), "the composefont parent: {preamble}");
        // Code = CID = gid for a promoted font; gids 1 and 2 shown as 2-byte
        // codes, big-endian, octal-escaped in the show string.
        assert!(out.contains("/PdfCraftF0 findfont 1000 scalefont setfont\n"), "font selected: {out}");
        assert!(out.contains("(\\000\\001\\000\\002)"), "2-byte codes for gids 1 and 2: {out}");
        assert_eq!(out.matches("gsave").count(), out.matches("grestore").count(), "balanced wrap: {out}");
    }

    /// A native CID-keyed CFF (`/FontFile3 /CIDFontType0C` under a Type0)
    /// emits codes that are the *font's* CIDs — the charset inverse, not the
    /// gids: content CIDs 10/20 land in gids 1/2, and the page shows
    /// `(\000\012\000\024)`. The PDF descendant's `/W` and `/CIDSystemInfo`
    /// have no home in the ReadData form (the near-passthrough decision), so
    /// none is emitted.
    #[test]
    fn type0_cidfonttype0_uses_font_cids() {
        let pdf = type0_pdf(
            "PdfCraftCID",
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /PdfCraftCID /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /FontDescriptor 5 0 R /DW 1000 /W [1 [500]] >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftCID /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            "/Subtype /CIDFontType0C",
            &cid_cff_program(),
            // Identity-H over 2-byte codes: CIDs 10 and 20 are the font's.
            b"BT /F1 24 Tf 20 100 Td <000A0014> Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the native CID arm degrades nothing: {notes:?}");
        assert!(preamble.contains("begincmap\n"), "the CMap body: {preamble}");
        assert!(preamble.contains("/PdfCraftD0 currentfile /ASCIIHexDecode filter //true //true ReadData\n"), "the ReadData route: {preamble}");
        assert!(out.contains("/PdfCraftF0 findfont 1000 scalefont setfont\n"), "composite selected: {out}");
        assert!(out.contains("(\\000\\012\\000\\024)"), "codes are the font's CIDs (10, 20), not the gids: {out}");
        assert!(!preamble.contains("/W [1 [500]]"), "/W has no home in the ReadData form: {preamble}");
        assert!(!preamble.contains("Japan1"), "the PDF's CIDSystemInfo has no home in the ReadData form: {preamble}");
    }

    /// A Type0 over a CIDFontType2 descendant with an embedded TrueType
    /// program emits the type-11 CIDFontType 2: `/CIDFontType 2`,
    /// `/CIDToGIDMap /Identity`, the identity `/CIDMap`, and the sfnts hex;
    /// the PDF descendant's `/W` rides verbatim and the codes are the CIDs
    /// (== gids under the identity map).
    #[test]
    fn type0_cidfonttype2_embeds_sfnts() {
        let ttf = ttf_program();
        let pdf = type0_pdf(
            "PdfCraftTT",
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /PdfCraftTT /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 /W [1 [600] 2 [700]] /CIDToGIDMap /Identity >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>",
            "",
            &ttf,
            b"BT /F1 24 Tf 20 100 Td <00010002> Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the CIDFontType 2 arm degrades nothing: {notes:?}");
        assert!(preamble.contains("%%BeginResource: cidfont PdfCraftD0\n"), "the descendant resource: {preamble}");
        assert!(preamble.contains("/CIDFontType 2 def\n"), "a CIDFontType 2 dict: {preamble}");
        assert!(preamble.contains("/CIDToGIDMap /Identity def\n"), "identity CID-to-GID mapping: {preamble}");
        assert!(preamble.contains("/CIDMap <000000010002> def\n"), "the identity CIDMap over the 3-glyph fixture: {preamble}");
        assert!(preamble.contains("/W [1 [600] 2 [700]] def\n"), "the PDF's /W carried verbatim: {preamble}");
        assert!(preamble.contains(&crate::fonts::hex_payload(&ttf)), "the whole TrueType program in sfnts hex: {preamble}");
        assert!(preamble.contains("/PdfCraftF0 /PdfCraftM0 [/PdfCraftD0] composefont pop\n"), "the composefont parent: {preamble}");
        assert!(out.contains("/PdfCraftF0 findfont 1000 scalefont setfont\n"), "composite selected: {out}");
        assert!(out.contains("(\\000\\001\\000\\002)"), "codes are the CIDs (== gids): {out}");
    }

    /// gid 0 gets a non-zero composite code — the highest unused one, 0xFFFF
    /// — and keeps it on the second call; a later gid keeps its own CID as
    /// its code.
    #[test]
    fn notdef_gid_gets_a_nonzero_composite_code() {
        let mut table = FontTable::default();
        let key = 42u128;
        table.order.push(key);
        table.fonts.insert(
            key,
            FontEntry {
                kind: EmbeddedKind::Cff { bytes: cff_program(), cid_keyed: false },
                subtype: Some("Type1".into()),
                descendant_subtype: None,
                base_font: Some("PdfCraftCFF".into()),
                bbox: None,
                csi: None,
                w: None,
                decision: Some(Decision::CidFontType0 { cids: None }),
                slices: Vec::new(),
                cid_codes: HashMap::new(),
                composite: None,
            },
        );
        let zero = table.cid_code(key, 0);
        assert_ne!(zero, 0, "gid 0 must not ride code 0");
        assert_eq!(zero, 0xFFFF, "the highest unused code: {zero}");
        assert_eq!(table.cid_code(key, 0), zero, "the remap is stable");
        assert_eq!(table.cid_code(key, 5), 5, "a later gid keeps CID == gid");
        // The preamble's CMap maps the remapped code back to CID 0.
        let preamble = String::from_utf8_lossy(&table.preamble()).into_owned();
        assert!(preamble.contains("1 begincidchar\n<FFFF> 0\nendcidchar\n"), "the remap rides as a cidchar: {preamble}");
    }

    /// A Type0 whose descendant carries no descriptor (no program at all)
    /// vectorizes with the existing CID-font note.
    #[test]
    fn cid_font_without_program_vectorizes() {
        let pdf = type0_pdf(
            "PdfCraftCID",
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /PdfCraftCID /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> >>",
            "<< >>",
            "",
            &[],
            b"BT /F1 24 Tf 20 100 Td <0001> Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("CID font program")), "the existing note path holds: {notes:?}");
        assert!(out.contains("moveto\n") || out.contains("fill\n"), "substitute outline drawn: {out}");
        assert!(!out.contains("findfont"), "no font machinery: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no resource: {preamble}");
    }

    /// A FontFile3 with `/Subtype /OpenType` stays vectorized with a note
    /// naming the family and the wrapper.
    #[test]
    fn open_type_cff_stays_vectorized() {
        let pdf = font_pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftOT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R /Encoding /StandardEncoding >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftOT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            "/Subtype /OpenType",
            &cff_program(),
            b"BT /F1 24 Tf 20 100 Td (A) Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("OpenType-wrapped CFF") && n.contains("PdfCraftOT")), "note names family and wrapper: {notes:?}");
        assert!(out.contains("curveto\n"), "glyph outline as path ops: {out}");
        assert!(!out.contains("findfont") && !out.contains("scalefont"), "no font machinery: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
    }

    /// Composite codes emit big-endian: a sparse-CFF fixture whose content
    /// reaches gids `0x0102` and `0x0007` shows the bytes `\001\002\000\007`
    /// in that order inside one run.
    #[test]
    fn composite_codes_emit_big_endian() {
        let pdf = font_pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftCFF /FirstChar 65 /LastChar 67 /Widths [600 700 700] /FontDescriptor 5 0 R /Encoding /StandardEncoding >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftCFF /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            "/Subtype /Type1C",
            &cff_program_sparse(),
            // B → gid 258 (0x0102), C → gid 7 (0x0007).
            b"BT /F1 24 Tf 20 100 Td (BC) Tj ET",
        );
        let (out, notes, _preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "no degradations: {notes:?}");
        assert!(out.contains("(\\001\\002\\000\\007)"), "big-endian 2-byte codes in order: {out}");
        assert_eq!(out.matches("xyshow").count(), 1, "one batched run: {out}");
    }

    /// A non-embedded font vectorizes (hayro's substitute outlines — the same
    /// shapes PdfCraft itself renders), with the drop recorded.
    #[test]
    fn non_embedded_font_vectorizes() {
        let pdf = font_pdf("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", "<< >>", "", &[], b"BT /F1 24 Tf 20 100 Td (P) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("not embedded")), "noted: {notes:?}");
        assert!(out.contains("fill\n") && out.contains("moveto\n"), "substitute outline drawn: {out}");
        assert!(!out.contains("findfont"), "no font: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no resource: {preamble}");
    }

    /// Consecutive same-font, same-basis glyphs batch into one `xyshow` — across
    /// a `Td`, whose translation the offsets absorb; a basis change (new font
    /// size) flushes and starts a new run. (Fixture: the embedded Type 1, whose
    /// A/B advances are 600/700 — the TrueType arm emits Type 42 since Layer B.)
    #[test]
    fn glyph_runs_batch_into_show() {
        let pdf =
            font_pdf(T1_FONT, T1_DESC, "", &t1_program(), b"BT /F1 24 Tf 20 100 Td (AB) Tj 0 -30 Td (AB) Tj ET\nBT /F1 12 Tf 20 150 Td (AB) Tj ET");
        let (out, notes, _preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "no degradations: {notes:?}");
        assert_eq!(out.matches("xyshow").count(), 2, "two runs, not four glyphs: {out}");
        assert_eq!(out.matches("moveto").count(), 2, "one origin per run: {out}");
        // `Td` is relative to the line matrix, so the second line restarts at
        // x = 20: the mid-run offset lands at 20 − 34.4 = −14.4 pt → −600 in
        // glyph units, and the Td translation at −30 pt → −1250.
        assert!(out.contains("(\\001\\002\\001\\002) [600 0 -600 -1250 600 0 0 0] xyshow\n"), "offsets from observed transforms: {out}");
    }

    /// A truncated FontFile parses nowhere: the glyphs fall back to hayro's
    /// substitute outlines, are vectorized, and the export completes with a note.
    #[test]
    fn corrupt_fontfile_falls_back_and_notes() {
        let truncated = &t1_program()[..600];
        let pdf = font_pdf(T1_FONT, T1_DESC, "", truncated, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET\n1 0 0 rg\n20 20 50 50 re f");
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("unreadable")), "noted: {notes:?}");
        assert!(out.contains("moveto\n") && out.contains("fill\n"), "substitute outlines drawn: {out}");
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text machinery: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
        assert!(out.contains("1 0 0 setrgbcolor\n"), "export continues after the corrupt font: {out}");
    }

    /// Character codes are assigned sequentially 0..=255 per font and spill into
    /// a second pseudo-font past 256 glyphs.
    #[test]
    fn more_than_256_glyphs_spills_to_second_font() {
        let mut table = FontTable::default();
        let key = 42u128;
        let mut codes = Vec::new();
        for gid in 0..300u32 {
            codes.push(table.char_code(key, gid));
        }
        let mut expected: Vec<u8> = (1..=255u8).collect();
        expected.extend(1..=45u8);
        assert_eq!(codes, expected, "sequential codes from 1, wrapping to a new slice");
        let names = table.slice_names(key);
        assert_eq!(names, ["PdfCraftF0".to_string(), "PdfCraftF1".to_string()], "a second pseudo-font exists");
        // Repeated glyphs reuse their slice's code.
        assert_eq!(table.char_code(key, 7), 8);
        assert_eq!(table.slice_index(key, 299).and_then(|i| names.get(i)).map(String::as_str), Some("PdfCraftF1"));
    }

    /// Each 256-code slice synthesizes its own cmap: driving `FontTable`
    /// directly (a fixture font drawn with >255 distinct glyphs is impractical
    /// to synthesize), 300 assigned codes produce two Type 42 resources whose
    /// sfnts map slice-0 codes and slice-1 codes to their gids respectively —
    /// parsed from the format-4 bytes of each embedded blob. Slice 0's code 1
    /// (the `.notdef` gid 0) carries no mapping: format 4 cannot resolve it.
    #[test]
    fn type42_second_slice_gets_own_cmap() {
        let mut table = FontTable::default();
        let key = 42u128;
        table.order.push(key);
        table.fonts.insert(
            key,
            FontEntry {
                kind: EmbeddedKind::TrueType(ttf_program_with_glyphs(400)),
                subtype: Some("TrueType".into()),
                descendant_subtype: None,
                base_font: Some("PdfCraftTT".into()),
                bbox: Some([0.0, -200.0, 700.0, 800.0]),
                csi: None,
                w: None,
                decision: Some(Decision::Type42),
                slices: Vec::new(),
                cid_codes: HashMap::new(),
                composite: None,
            },
        );
        for gid in 0..300u32 {
            table.char_code(key, gid);
        }
        let preamble = String::from_utf8_lossy(&table.preamble()).into_owned();
        assert_eq!(preamble.matches("%%BeginResource: font").count(), 2, "one resource per slice: {preamble}");
        assert!(
            preamble.contains("%%BeginResource: font PdfCraftF0\n") && preamble.contains("%%BeginResource: font PdfCraftF1\n"),
            "both slices defined: {preamble}"
        );
        assert_eq!(preamble.matches("/FontType 42 def").count(), 2, "both are Type 42: {preamble}");
        let blobs = sfnts_blobs(&preamble);
        assert_eq!(blobs.len(), 2, "one sfnts string per slice");
        // Slice 0: codes 2..=255 → gids 1..=254 (code 1's gid 0 is dropped).
        assert_eq!(format4_gid(&blobs[0], 1), None, "the .notdef code carries no mapping");
        assert_eq!(format4_gid(&blobs[0], 2), Some(1), "slice 0's first code → its gid");
        assert_eq!(format4_gid(&blobs[0], 255), Some(254), "slice 0's last code → its gid");
        // Slice 1: codes 1..=45 → gids 255..=299.
        assert_eq!(format4_gid(&blobs[1], 1), Some(255), "slice 1's first code → its gid");
        assert_eq!(format4_gid(&blobs[1], 45), Some(299), "slice 1's last code → its gid");
    }

    /// A name the font's `post` table duplicates across two used gids goes to
    /// the lower gid, deterministically: the name assignment runs over
    /// gid-sorted pairs, so whichever gid the `codes` `HashMap` happens to
    /// yield first cannot pick the winner — otherwise the same input PDF
    /// would export to different bytes across runs (random dedup fallbacks).
    /// Fixture: a post 2.0 table whose gids 1 and 2 both point at one pool
    /// name (legal in post 2.0); codes 1 and 2 assigned to gids 1 and 2.
    #[test]
    fn duplicate_post_names_go_to_the_lower_gid() {
        // post 2.0 over three glyphs: the 32-byte header, numberOfNewGlyphs,
        // glyphNameIndex [.notdef(0), 258, 258] and one Pascal-string pool
        // entry "box" — so gids 1 and 2 share the name.
        let mut post = Vec::new();
        post.extend_from_slice(&0x0002_0000u32.to_be_bytes()); // version 2.0
        post.extend_from_slice(&0u32.to_be_bytes()); // italicAngle
        post.extend_from_slice(&(-100i16).to_be_bytes()); // underlinePosition
        post.extend_from_slice(&50u16.to_be_bytes()); // underlineThickness
        post.extend_from_slice(&0u32.to_be_bytes()); // isFixedPitch
        post.extend_from_slice(&[0; 16]); // memory limits
        post.extend_from_slice(&3u16.to_be_bytes()); // numberOfNewGlyphs
        post.extend_from_slice(&[0u16, 258, 258].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>());
        post.extend_from_slice(&[3, b'b', b'o', b'x']); // the "box" pool string
        let mut table = FontTable::default();
        let key = 42u128;
        table.order.push(key);
        table.fonts.insert(
            key,
            FontEntry {
                kind: EmbeddedKind::TrueType(ttf_program_with_post(3, post)),
                subtype: Some("TrueType".into()),
                descendant_subtype: None,
                base_font: Some("PdfCraftTT".into()),
                bbox: None,
                csi: None,
                w: None,
                decision: Some(Decision::Type42),
                slices: Vec::new(),
                cid_codes: HashMap::new(),
                composite: None,
            },
        );
        assert_eq!(table.char_code(key, 1), 1, "gid 1 takes code 1");
        assert_eq!(table.char_code(key, 2), 2, "gid 2 takes code 2");
        let preamble = String::from_utf8_lossy(&table.preamble()).into_owned();
        assert!(preamble.contains("/.notdef /box /g0002 /.notdef"), "Encoding: code 1 keeps the shared name, code 2 takes the fallback: {preamble}");
        assert!(preamble.contains("/box 1 def\n"), "CharStrings: the shared name maps to the lower gid: {preamble}");
        assert!(preamble.contains("/g0002 2 def\n"), "CharStrings: the higher gid keeps its own fallback entry: {preamble}");
    }

    /// `composite_cycle_falls_back_to_whole_font`, the export half: a font
    /// whose composite glyph references itself still exports as Type 42 — the
    /// surgery declines the subset (`None`) and the emitter falls back to the
    /// whole program, so the sfnts carry every glyph's data exactly as the
    /// un-subsetted synthesis would, with no degradation note (correct
    /// output, only larger).
    #[test]
    fn composite_cycle_falls_back_to_whole_font() {
        let ttf = composite_ttf_program(true);
        assert!(sfnt::subset_glyphs(&ttf, &[1, 3]).is_none(), "the unit half: the cycle declines the subset");
        let pdf = font_pdf(TT_FONT, TT_DESC, "", &ttf, b"BT /F1 24 Tf 20 100 Td (AC) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "the whole-font fallback is silent — correct, only larger");
        assert!(preamble.contains("/FontType 42 def\n"), "the Type 42 dictionary emits: {preamble}");
        assert!(out.contains("xyshow"), "the export still draws text through the font: {out}");
        let blobs = sfnts_blobs(&preamble);
        assert_eq!(blobs.len(), 1, "one slice, one sfnts string: {preamble}");
        assert_eq!(
            blobs[0],
            sfnt::synthesize_cmap(&ttf, &[(1, 1), (2, 3)]).expect("the whole-font synthesis succeeds"),
            "the sfnts carry the whole font — every glyph's data — identically to the un-subsetted pipeline"
        );
    }

    /// The W1×W3 seam contract (final review, finding 1): the Type 42
    /// pipeline — subset, then synthesize the cmap over the subset — defines
    /// nothing only when the whole-program synthesis declines too, so the
    /// page's `findfont` cannot miss. Here the drawn set includes a cyclic
    /// composite: the strategy rehearsal passes over the whole program while
    /// the subset declines, and the export still defines the resource with a
    /// WORKING cmap — a consumer resolves each assigned code to its gid
    /// through the emitted (3,0) subtable — and notes nothing (correct
    /// output, only larger).
    #[test]
    fn type42_declined_subset_still_defines_working_cmap() {
        let ttf = composite_ttf_program(true);
        assert!(type42_ready(&ttf), "the strategy rehearsal passes over the whole program");
        assert!(sfnt::subset_glyphs(&ttf, &[1, 3]).is_none(), "the unit half: the cycle declines the subset");
        let pdf = font_pdf(TT_FONT, TT_DESC, "", &ttf, b"BT /F1 24 Tf 20 100 Td (AC) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "a silent fallback is correct, only larger");
        assert!(preamble.contains("%%BeginResource: font PdfCraftF0\n"), "the resource the page findfonts is defined: {preamble}");
        assert!(out.contains("/PdfCraftF0 findfont"), "the page draws through it: {out}");
        let blobs = sfnts_blobs(&preamble);
        assert_eq!(blobs.len(), 1, "one slice, one sfnts string: {preamble}");
        assert_eq!(format4_gid(&blobs[0], 1), Some(1), "code 1 → gid 1 through the emitted cmap");
        assert_eq!(format4_gid(&blobs[0], 2), Some(3), "code 2 → gid 3 through the emitted cmap");
        assert_eq!(format4_gid(&blobs[0], 3), None, "an unassigned code stays unmapped");
    }

    /// The Type 42 name tables stay valid over the subset: `/Encoding` and
    /// `/CharStrings` are built from the ORIGINAL program's gids, and the
    /// surgery's stable numbering is what makes that survive — the names
    /// resolve to glyphs that still hold their outlines in the subsetted
    /// sfnts, including the pure component gid the dictionary never names
    /// (it rides only as the composite's outline data).
    #[test]
    fn type42_names_stay_valid_over_the_subset() {
        let ttf = composite_ttf_program(false);
        let pdf = font_pdf(TT_FONT, TT_DESC, "", &ttf, b"BT /F1 24 Tf 20 100 Td (AC) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "subsetting degrades nothing");
        assert!(out.contains("xyshow"), "the text draws through the font: {out}");
        // post 3.0: every name is the /gNNNN fallback, keyed by the ORIGINAL
        // gid — unchanged by the subset, by construction.
        assert!(preamble.contains("/g0001 1 def\n"), "CharStrings names gid 1 under its original id: {preamble}");
        assert!(preamble.contains("/g0003 3 def\n"), "CharStrings names the composite gid 3 under its original id: {preamble}");
        // ...and the subset keeps those gids alive, the pure component included.
        let blobs = sfnts_blobs(&preamble);
        assert_eq!(blobs.len(), 1, "one slice, one sfnts string: {preamble}");
        let blob = &blobs[0];
        let (off, _len) = dir_entry(blob, b"maxp").expect("a maxp directory entry");
        assert_eq!(be16_at(blob, off + 4), Some(5), "numGlyphs unchanged by the subset");
        let (off, len) = dir_entry(blob, b"loca").expect("a loca directory entry");
        let loca = blob.get(off..off + len).expect("the loca table within the blob");
        let offs: Vec<usize> = (0..len / 2).map(|i| usize::from(be16_at(loca, i * 2).expect("a loca entry")) * 2).collect();
        assert_eq!(offs.len(), 6, "five glyphs plus the final end");
        assert!(offs[2] > offs[1] && offs[3] > offs[2], "the component gids 1 and 2 hold their outlines");
        assert!(offs[4] > offs[3], "the composite gid 3 holds its outline");
        assert_eq!(offs[5], offs[4], "the unused gid 4 empties");
    }

    // --------------------------------------------------------------- extras

    /// A Type 3 glyph's PDF drawing instructions are re-interpreted through the
    /// device: ordinary path ops, no font machinery, no note.
    #[test]
    fn type3_glyph_interprets_to_paths() {
        let pdf = font_pdf(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 100 100] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /box 6 0 R >> /Encoding << /Differences [65 /box] >> /FirstChar 65 /LastChar 65 /Widths [600] >>",
            "<< >>",
            "",
            b"0 0 100 100 re f",
            b"BT /F1 2400 Tf 20 100 Td (A) Tj ET",
        );
        let (out, notes, _preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "Type 3 interpretation is faithful: {notes:?}");
        assert!(out.contains("moveto\n") && out.contains("fill\n"), "glyph program drawn: {out}");
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text machinery: {out}");
    }

    /// Point `tag`'s sfnt directory entry past the end of the program: the
    /// table becomes unreadable to skrifa while the rest (notably `head`)
    /// still parses — exactly the gap between the old CIDFontType 2 strategy
    /// gate (`head` only) and the preamble's `maxp` read that review round 1
    /// closed.
    fn bury_table(font: &[u8], tag: &[u8; 4]) -> Vec<u8> {
        let mut out = font.to_vec();
        let Some(num) = out.get(4..6).map(|b| usize::from(u16::from_be_bytes(b.try_into().expect("two bytes")))) else {
            return out;
        };
        for i in 0..num {
            let at = 12 + i * 16;
            let past_end = (out.len() + 4096) as u32;
            if out.get(at..at + 4) == Some(&tag[..])
                && let Some(slot) = out.get_mut(at + 8..at + 12)
            {
                slot.copy_from_slice(&past_end.to_be_bytes());
            }
        }
        out
    }

    /// An empty `/W` — outer or nested — carries no width entries, and a
    /// level with no objects must never emit an unbalanced `]` (a gs error
    /// that would kill the whole program): the CIDFontType 2 emission falls
    /// back to `/DW 1000`.
    #[test]
    fn empty_w_array_falls_back_to_dw() {
        for w in ["/W []", "/W [1 []]"] {
            let pdf = type0_pdf(
                "PdfCraftTT",
                &format!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /PdfCraftTT /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 {w} /CIDToGIDMap /Identity >>"
                ),
                "<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>",
                "",
                &ttf_program(),
                b"BT /F1 24 Tf 20 100 Td <00010002> Tj ET",
            );
            let (_out, notes, preamble) = driven(&pdf);
            assert_eq!(notes.join("\n"), "", "the fallback is not a degradation: {notes:?}");
            assert!(preamble.contains("/DW 1000 def\n"), "'{w}' falls back to /DW 1000: {preamble}");
            assert!(!preamble.contains("/W ]") && !preamble.contains("]]"), "no unbalanced bracket from '{w}': {preamble}");
        }
    }

    /// The CIDFontType 2 gate covers the preamble's whole parse: a program
    /// whose `head` reads but whose `maxp` does not declines at strategy
    /// time — vectorize with the family-naming note, no resource, no
    /// `findfont` — instead of emitting a page that references a font that
    /// never got defined.
    #[test]
    fn maxp_less_cidfonttype2_declines_at_strategy_time() {
        let buried = bury_table(&ttf_program(), b"maxp");
        // Unit half: the gate itself sees what the preamble will see.
        assert!(cidfont2_ready(&ttf_program()), "the intact fixture passes");
        assert!(!cidfont2_ready(&buried), "head parses, maxp does not: the gate must decline");
        // Driven half: the strategy arm notes and vectorizes.
        let pdf = type0_pdf(
            "PdfCraftTT",
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /PdfCraftTT /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 /W [1 [600]] /CIDToGIDMap /Identity >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>",
            "",
            &buried,
            b"BT /F1 24 Tf 20 100 Td <00010002> Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert!(
            notes.iter().any(|n| n.contains("CID TrueType") && n.contains("PdfCraftTT") && n.contains("unreadable")),
            "note names family and reason: {notes:?}"
        );
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text-as-text: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource for a program the preamble cannot parse: {preamble}");
    }

    /// Stroked and invisible text never becomes `show` text: stroke modes
    /// vectorize (the outline is stroked like any path), invisible is skipped.
    #[test]
    fn stroke_text_vectorizes_invisible_skips() {
        let pdf = font_pdf(T1_FONT, T1_DESC, "", &t1_program(), b"BT /F1 24 Tf 1 Tr 20 100 Td (A) Tj ET\nBT /F1 24 Tf 3 Tr 20 100 Td (A) Tj ET");
        let (out, _notes, _preamble) = driven(&pdf);
        assert!(out.contains("stroke\n"), "stroked text as stroked outline: {out}");
        assert!(!out.contains("findfont") && !out.contains("xyshow"), "no text machinery: {out}");
        assert_eq!(out.matches("stroke\n").count(), 1, "invisible text draws nothing: {out}");
    }
}
