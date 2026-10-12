//! Text emission: Type 1 passthrough, Type 42 and vectorized fallback (Layer A, Task 5).
//!
//! Glyphs reach the device one at a time; text leaves as text whenever the PDF
//! embeds a font program PostScript can execute natively:
//!
//! - **Type 1** (`/FontFile`): the font program's bytes land in the prolog verbatim
//!   (they are a complete, self-defining PostScript font), then a copy is defined
//!   under our own name with the PDF's `/FontBBox` and a re-built `/Encoding` that
//!   maps our sequential character codes to the program's glyph names.
//! - **TrueType** (`/FontFile2`, `/Subtype /TrueType`): a Type 42 font dictionary
//!   built in the prolog, with the raw font bytes as `/sfnts` and an `/Encoding`
//!   that maps our codes to `/glyphNNNN` names (the Type 42 by-index convention).
//!   `/GlyphNames2Unicode` is omitted (Layer A; text-extraction parity is later).
//! - **Everything else** — CFF (`/FontFile3`), CID/Type0, Type 3, non-embedded and
//!   unreadable font programs — is vectorized through the Task 3 path machinery
//!   (`outline()` → path ops under the glyph transform), with one fidelity note
//!   per font recording why. Type 3 glyphs are the exception: their PDF drawing
//!   instructions are re-interpreted through the device itself, which is faithful.
//!
//! Geometry: the interpreter hands each glyph `transform * glyph_transform`, the
//! total map from 1000-unit glyph space to the page (findings (b)). Consecutive
//! glyphs of one font whose linear part is bit-identical (true within a text run:
//! text-matrix advances are pure translations, which compose exactly) batch into
//! a single `xyshow`: the CTM becomes the run's linear part, the font is scaled
//! so its effective glyph matrix is the identity on 1000-unit space (`1000
//! scalefont` over the program's natural matrix), and every glyph origin comes
//! from the observed transforms, so positioning never depends on the PDF's
//! declared widths agreeing with the font's.

// The first live constructor arrives with Task 7's `export()` assembly; until then
// only the tests build a FontTable, so the emission chain is dead code otherwise.
#![allow(dead_code)]

use std::collections::HashMap;

use hayro::hayro_interpret::CacheKey;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, Name, Object, Stream};
use hayro::hayro_interpret::hayro_syntax::page::Resources;

use crate::psnum;

/// How glyphs of one font are emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextStrategy {
    /// Embedded Type 1: program bytes in the prolog, re-encoded copy, `show` text.
    Type1Passthrough,
    /// Embedded TrueType: Type 42 dictionary with `sfnts`, `show` text.
    Type42,
    /// Outline glyphs as plain path operations.
    Vectorize,
}

/// Which font program (if any) the PDF embedded for one font dict.
#[derive(Clone, Debug)]
enum EmbeddedKind {
    /// `/FontFile` — a Type 1 font program, decoded.
    Type1(Vec<u8>),
    /// `/FontFile2` — TrueType data, decoded.
    TrueType(Vec<u8>),
    /// `/FontFile3` — CFF or OpenType (never executed as text in Layer A).
    Cff,
    /// No embedded program.
    None,
}

/// The memoized decision for one font, with whatever the emission needs.
enum Decision {
    /// The parsed Type 1 program: its own name (for `findfont`) and gid → glyph name.
    Type1 { name: String, names: HashMap<u32, String> },
    /// The TrueType units-per-em, needed for the Type 42 `/FontMatrix`.
    Type42 { upem: u32 },
    /// Path fallback; the note (if any) was queued when this was decided.
    Vectorize,
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
    /// `/FontDescriptor /FontBBox` (or the font dict's own, for Type 3).
    bbox: Option<[f64; 4]>,
    decision: Option<Decision>,
    slices: Vec<Slice>,
}

impl FontEntry {
    /// The entry for a font the resource walk never saw (nested resources, or a
    /// fallback font with no dict of its own).
    fn unknown() -> Self {
        Self { kind: EmbeddedKind::None, subtype: None, bbox: None, decision: None, slices: Vec::new() }
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
}

impl FontTable {
    /// Walk the page's font resources (own and inherited) and extract, per font
    /// dict, the embedded program bytes, subtype and bounding box. Everything is
    /// copied, so the table outlives the PDF objects.
    pub(crate) fn from_resources(resources: &Resources<'_>) -> Self {
        let mut table = Self::default();
        let mut level = Some(resources.clone());
        while let Some(res) = level {
            for key in res.fonts.keys() {
                let Some(dict) = res.fonts.get::<Dict<'_>>(key.as_ref()) else {
                    continue;
                };
                let cache_key = CacheKey::cache_key(&dict);
                if table.fonts.contains_key(&cache_key) {
                    continue;
                }
                table.order.push(cache_key);
                table.fonts.insert(cache_key, entry_of(&dict));
            }
            level = res.parent().cloned();
        }
        table
    }

    /// Decide (once per font) how its glyphs are emitted. Queues a fidelity note
    /// when the decision degrades.
    pub(crate) fn strategy(&mut self, glyph: &Glyph<'_>) -> TextStrategy {
        let Glyph::Outline(outline) = glyph else {
            // Type 3 glyphs have no font program; the device re-interprets them.
            return TextStrategy::Vectorize;
        };
        let key = outline.font_cache_key();
        let has_font_data = outline.font_data().is_some();
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
        let decision = match (entry.map(|e| &e.kind), entry.and_then(|e| e.subtype.as_deref())) {
            (_, Some("Type0")) => {
                self.pending_notes.push("text vectorized (CID font program)".into());
                Decision::Vectorize
            }
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
            (Some(EmbeddedKind::TrueType(data)), Some("TrueType")) if has_font_data => match read_upem(data) {
                Some(upem) if (16..=16384).contains(&upem) => Decision::Type42 { upem },
                _ => {
                    self.pending_notes.push("text vectorized (font program unreadable)".into());
                    Decision::Vectorize
                }
            },
            (Some(EmbeddedKind::TrueType(_)), _) => {
                // The bytes exist but hayro did not load them (the glyph ids would
                // belong to a substitute font), or the subtype is unexpected.
                self.pending_notes.push("text vectorized (font program unreadable)".into());
                Decision::Vectorize
            }
            (Some(EmbeddedKind::Cff), _) => {
                self.pending_notes.push("text vectorized (CFF font program)".into());
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
            Some(Decision::Type42 { .. }) => TextStrategy::Type42,
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

    /// Take the notes queued since the last drain.
    pub(crate) fn drain_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notes)
    }

    /// The font definitions for the prolog (`%%BeginResource` per PostScript
    /// font, in first-use order). Vectorized fonts define nothing.
    pub(crate) fn preamble(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for key in &self.order {
            let Some(entry) = self.fonts.get(key) else {
                continue;
            };
            match entry.decision.as_ref() {
                Some(Decision::Type1 { name, names }) => {
                    let EmbeddedKind::Type1(program) = &entry.kind else { continue };
                    for slice in &entry.slices {
                        type1_resource(&mut out, slice, name, names, program, entry.bbox);
                    }
                }
                Some(Decision::Type42 { upem }) => {
                    let EmbeddedKind::TrueType(data) = &entry.kind else { continue };
                    for slice in &entry.slices {
                        type42_resource(&mut out, slice, *upem, entry.bbox, data);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// One re-encoded copy of the verbatim Type 1 program.
fn type1_resource(out: &mut Vec<u8>, slice: &Slice, original: &str, names: &HashMap<u32, String>, program: &[u8], bbox: Option<[f64; 4]>) {
    out.extend_from_slice(format!("%%BeginResource: font {}\n", slice.name).as_bytes());
    emit_program_bytes(out, program);
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

/// One Type 42 dictionary built from raw TrueType bytes.
fn type42_resource(out: &mut Vec<u8>, slice: &Slice, upem: u32, bbox: Option<[f64; 4]>, data: &[u8]) {
    out.extend_from_slice(format!("%%BeginResource: font {}\n", slice.name).as_bytes());
    out.extend_from_slice(format!("/{n} 10 dict dup begin\n/FontName /{n} def\n/FontType 42 def\n/PaintType 0 def\n", n = slice.name).as_bytes());
    out.extend_from_slice(format!("/FontMatrix [1 {upem} div 0 0 1 {upem} div 0 0] def\n").as_bytes());
    match bbox {
        Some(bbox) => out.extend_from_slice(format!("/FontBBox {} def\n", bbox_text(bbox)).as_bytes()),
        // No PDF bbox: generous and never clipping (only font-cache sizing reads it).
        None => out.extend_from_slice(format!("/FontBBox [-{u} -{u} {u} {u}] def\n", u = 2 * upem).as_bytes()),
    }
    out.extend_from_slice(b"/Encoding [");
    encoding_names(out, &slice.codes.iter().map(|(gid, code)| (*code, format!("glyph{gid}"))).collect::<Vec<_>>());
    out.extend_from_slice(b"] def\n");
    // Ghostscript's Type 42 machinery also wants a /CharStrings dict mapping
    // the encoding names to glyph indices (name → integer). Fonts without it
    // are rejected at `definefont` there (verified; ps2write emits one too).
    let mut gids = slice.codes.keys().copied().collect::<Vec<_>>();
    gids.sort_unstable();
    out.extend_from_slice(b"/CharStrings 256 dict dup begin\n/.notdef 0 def\n");
    for gid in gids {
        out.extend_from_slice(format!("/glyph{gid} {gid} def\n").as_bytes());
    }
    out.extend_from_slice(b"end def\n/sfnts [\n");
    // Chunked per the Type 42 convention: at most 65535 bytes per string, every
    // chunk but the last a multiple of four.
    const CHUNK: usize = 65520;
    for (i, chunk) in data.chunks(CHUNK).enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(ps_string(chunk).as_bytes());
    }
    out.extend_from_slice(b"] def\ncurrentdict end definefont pop\n%%EndResource\n");
}

/// Build the walk result for one font dict.
fn entry_of(dict: &Dict<'_>) -> FontEntry {
    let kind = dict
        .get::<Dict<'_>>(b"FontDescriptor")
        .and_then(|desc| {
            if let Some(stream) = desc.get::<Stream<'_>>(b"FontFile") {
                stream.decoded().ok().map(|d| EmbeddedKind::Type1(d.into_owned()))
            } else if let Some(stream) = desc.get::<Stream<'_>>(b"FontFile2") {
                stream.decoded().ok().map(|d| EmbeddedKind::TrueType(d.into_owned()))
            } else if desc.contains_key(b"FontFile3") {
                Some(EmbeddedKind::Cff)
            } else {
                Some(EmbeddedKind::None)
            }
        })
        .unwrap_or(EmbeddedKind::None);
    FontEntry {
        kind,
        subtype: dict.get::<Name<'_>>(b"Subtype").map(|n| n.as_str().to_string()),
        bbox: bbox_of(dict),
        decision: None,
        slices: Vec::new(),
    }
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

/// Parse a Type 1 font program into its own name and its glyph-name table — the
/// same parse hayro performs (same skrifa), so the glyph ids agree by construction.
fn parse_type1(program: &[u8]) -> Option<(String, HashMap<u32, String>)> {
    let font = skrifa::raw::ps::type1::Type1Font::new(program).ok()?;
    let name = font.name()?.to_string();
    let names = font.glyph_names().map(|(gid, name)| (gid.to_u32(), name.to_string())).collect();
    Some((name, names))
}

/// A TrueType font's units-per-em, from its `head` table (all reads bounded).
fn read_upem(data: &[u8]) -> Option<u32> {
    let num_tables = u16::from_be_bytes(data.get(4..6)?.try_into().ok()?);
    let dir = data.get(12..)?;
    for record in dir.as_chunks::<16>().0.iter().take(usize::from(num_tables)) {
        if record.starts_with(b"head") {
            let offset = usize::try_from(u32::from_be_bytes(record.get(8..12)?.try_into().ok()?)).ok()?;
            let at = offset.checked_add(18)?;
            return Some(u32::from(u16::from_be_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?)));
        }
    }
    None
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
/// The `%%BeginData` count covers the payload plus its terminating newline.
fn emit_program_bytes(out: &mut Vec<u8>, program: &[u8]) {
    let ascii = program.iter().all(|b| matches!(*b, b'\n' | b'\r' | b'\t' | 0x20..=0x7e));
    if ascii {
        out.extend_from_slice(program);
        out.push(b'\n');
    } else {
        out.extend_from_slice(format!("%%BeginData: {} 0 BinaryBytes\n", program.len() + 1).as_bytes());
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

    /// A minimal TrueType font: 1000 upem, three glyphs (empty outlines — these
    /// fixtures test text emission, not outlines), a (1,0) format-6 cmap mapping
    /// A→1 and B→2, and a version-3.0 `post` (no names, so `/glyphNNNN`
    /// addressing is the only route — exactly what the emitter uses).
    fn ttf_program() -> Vec<u8> {
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
        maxp.extend_from_slice(&be16(3)); // numGlyphs
        maxp.extend_from_slice(&[0; 26]);
        let mut cmap = Vec::new();
        cmap.extend_from_slice(&be16(0)); // version
        cmap.extend_from_slice(&be16(1)); // numTables
        cmap.extend_from_slice(&be16(1)); // platformID: Macintosh
        cmap.extend_from_slice(&be16(0)); // encodingID: Roman
        cmap.extend_from_slice(&be32(12)); // subtable offset
        cmap.extend_from_slice(&be16(6)); // format 6
        cmap.extend_from_slice(&be16(14)); // length
        cmap.extend_from_slice(&be16(0)); // language
        cmap.extend_from_slice(&be16(65)); // firstCode
        cmap.extend_from_slice(&be16(2)); // entryCount
        cmap.extend_from_slice(&be16(1)); // 'A' → gid 1
        cmap.extend_from_slice(&be16(2)); // 'B' → gid 2
        let mut post = Vec::new();
        post.extend_from_slice(&be32(0x0003_0000)); // version 3.0
        post.extend_from_slice(&be32(0)); // italicAngle
        post.extend_from_slice(&be16(0xFF9C)); // underlinePosition (-100)
        post.extend_from_slice(&be16(50)); // underlineThickness
        post.extend_from_slice(&be32(0)); // isFixedPitch
        post.extend_from_slice(&[0; 16]); // memory limits
        // Two identical simple glyphs (a 300×400 box at x=100), 34 bytes each;
        // glyph 0 (.notdef) is empty. Short `loca` holds byte offsets ÷ 2.
        fn box_glyph() -> Vec<u8> {
            let mut g = Vec::new();
            g.extend_from_slice(&be16(1)); // numberOfContours
            g.extend_from_slice(&be16(100)); // xMin
            g.extend_from_slice(&be16(0)); // yMin
            g.extend_from_slice(&be16(400)); // xMax
            g.extend_from_slice(&be16(400)); // yMax
            g.extend_from_slice(&be16(3)); // endPtsOfContours[0] (4 points)
            g.extend_from_slice(&be16(0)); // instructionLength
            g.extend_from_slice(&[0x01; 4]); // flags: on-curve, 16-bit deltas
            for dx in [100i16, 0, 300, 0] {
                g.extend_from_slice(&be16(dx as u16));
            }
            for dy in [0i16, 400, 0, -400] {
                g.extend_from_slice(&be16(dy as u16));
            }
            g
        }
        let glyph = box_glyph();
        let mut glyf = Vec::new();
        glyf.extend_from_slice(&glyph);
        glyf.extend_from_slice(&glyph);
        // loca: glyph0 empty at 0, glyphs 1-2 span the two boxes (÷2, short format).
        let loca = [0u16, 0, 17, 34].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>();

        // Directory entries, alphabetically by tag as the format requires.
        let tables: [(&[u8; 4], Vec<u8>); 8] =
            [(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"post", post)];
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

    // ------------------------------------------------------------------ drive

    /// Drive the device over one page of `pdf` and return (program, notes,
    /// preamble). The device is built with the page's font resources, matching
    /// how Task 7's `export()` will assemble it.
    fn driven(pdf: &[u8]) -> (String, Vec<String>, String) {
        let pdf = Pdf::new(pdf.to_vec()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 200.0, 300.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = PsDevice::with_fonts(None, page.resources());
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

    const T1_FONT: &str =
        "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftTest /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>";
    const T1_DESC: &str = "<< /Type /FontDescriptor /FontName /PdfCraftTest /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 6 0 R >>";
    const TT_FONT: &str =
        "<< /Type /Font /Subtype /TrueType /BaseFont /PdfCraftTT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>";
    const TT_DESC: &str = "<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>";

    // ------------------------------------------------------------------ tests

    /// An embedded Type 1 font's program bytes land in the prolog verbatim, with
    /// the PDF's `/FontBBox` and a re-encoded copy under our own font name.
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

    /// An embedded TrueType font is emitted as a Type 42: `sfnts` from the
    /// FontFile2 bytes, `/FontMatrix` from the units-per-em, `/Encoding` mapping
    /// assigned codes to `/glyphNNNN`, no `/GlyphNames2Unicode`.
    #[test]
    fn truetype_emits_type42_sfnts() {
        let ttf = ttf_program();
        let pdf = font_pdf(TT_FONT, TT_DESC, "", &ttf, b"BT /F1 24 Tf 20 100 Td (AB) Tj ET");
        let (out, notes, preamble) = driven(&pdf);
        assert_eq!(notes.join("\n"), "", "Type 42 degrades nothing");
        assert!(preamble.contains("/FontType 42 def\n"), "Type 42: {preamble}");
        assert!(preamble.contains("/FontMatrix [1 1000 div 0 0 1 1000 div 0 0] def\n"), "upem 1000 matrix: {preamble}");
        assert!(preamble.contains("/Encoding [\n/.notdef /glyph1 /glyph2 "), "codes map to glyph ids: {preamble}");
        assert!(!preamble.contains("GlyphNames2Unicode"), "omitted in Layer A: {preamble}");
        let expected = ps_string(&ttf);
        assert!(contains_bytes(&preamble, expected.as_bytes()), "sfnts carries the raw TrueType bytes: {preamble}");
        assert!(out.contains("(\\001\\002) [600 0 0 0] xyshow\n"), "text as text: {out}");
    }

    /// A CFF font program (`/FontFile3 /Type1C`) is vectorized: the glyph becomes
    /// path operators and no font is defined or selected.
    #[test]
    fn cff_font_vectorizes() {
        let pdf = font_pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftCFF /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R /Encoding /StandardEncoding >>",
            "<< /Type /FontDescriptor /FontName /PdfCraftCFF /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            "/Subtype /Type1C",
            &cff_program(),
            b"BT /F1 24 Tf 20 100 Td (A) Tj ET",
        );
        let (out, notes, preamble) = driven(&pdf);
        assert!(notes.iter().any(|n| n.contains("CFF")), "vectorization noted: {notes:?}");
        assert!(out.contains("curveto\n"), "glyph outline as path ops: {out}");
        assert!(!out.contains("findfont") && !out.contains("scalefont"), "no font machinery: {out}");
        assert!(!preamble.contains("%%BeginResource"), "no font resource: {preamble}");
        assert!(out.contains("fill\n"), "painted: {out}");
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
    /// size) flushes and starts a new run.
    #[test]
    fn glyph_runs_batch_into_show() {
        let pdf =
            font_pdf(TT_FONT, TT_DESC, "", &ttf_program(), b"BT /F1 24 Tf 20 100 Td (AB) Tj 0 -30 Td (AB) Tj ET\nBT /F1 12 Tf 20 150 Td (AB) Tj ET");
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
