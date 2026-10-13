//! CID-keyed emission: identity CMaps and CIDFont resources for the composite
//! arms (Layer B Task 4; emission shapes reworked in Task 5 to the forms this
//! repo's gs oracle actually executes).
//!
//! CFF and CID fonts re-emit as PostScript **Type 0 composites**: a CMap
//! resource mapping our assigned codes to the font's CIDs, a CIDFont
//! descendant resource embedding the font program, and a Type 0 parent tying
//! them together. A font's blocks go into the preamble in that order — CMap,
//! descendant, parent — the design's ordering rule.
//!
//! **Execution shapes, re-probed against gs 10.08.0 (build 2026-09-08) with
//! synthetic fixtures (black-box; Task 5's probes):**
//!
//! - **CMap body grammar is `begincidrange`/`begincidchar`, values plain
//!   integers.** Task 4 emitted `bfrange`/`bfchar` with hex-string values —
//!   the *ToUnicode* grammar. Those execute (the resource defines) but map no
//!   code to any CID: every `show` through the composite found no glyph and
//!   drew nothing (inkcov 0 for every code; swapping the mapping changed
//!   nothing). With `cidchar` the same block renders, and a swapped mapping
//!   changes the raster — the probe that pinned this. Adobe's own `Identity-H`
//!   uses `begincidrange` exclusively; the run-collapse emission below mirrors
//!   it (maximal consecutive code == cid runs become one cidrange, everything
//!   else rides as cidchars, ≤ [`GROUP`] per group).
//! - **CIDFontType 0 is gs's own CFF loader route**: `/FontSetInit /ProcSet
//!   findresource begin`, `/<name> currentfile /ASCIIHexDecode filter //true
//!   //true ReadData` with the whole bare CFF as following-line hex,
//!   terminated by `>`, then `pop`. ReadData promotes the bytes to a real
//!   CIDFont resource under `<name>` (name-keyed CFFs are renumbered so
//!   **CID == charstring index** — proven by byte-identical rasters against
//!   the promoted font's own numbering; CID-keyed CFFs keep their charset's
//!   CIDs — proven the same way). In that form `/DW`, `/W` and a supplied
//!   `/CIDSystemInfo` have **no home**: the resource is built from the CFF
//!   itself (gs derives Adobe/Identity for promoted fonts, the ROS for
//!   CID-keyed ones), which is the design spec's near-passthrough decision
//!   outranking its /W sentence. The `//true //true` operands are
//!   `forceresname` (our name wins over a subset font's internal name) and
//!   `forcecid` (the promotion itself). `ReadData` must be the **last token
//!   on its line** — the hex it pulls through the currentfile filter is
//!   whatever follows, so any token after it on the same line would be eaten
//!   as data; the `pop` (for the FontSet ReadData leaves) comes after the `>`
//!   terminator. Operand and dictionary depth were verified 0 at every block
//!   boundary.
//! - **CIDFontType 2 needs gs's type-11 key set around the PDF-transliterated
//!   ones**: the pinned keys alone fail `.buildfont11` (`/undefined in
//!   --.buildfont11--`); with `/CIDMap` (2 bytes per CID, big-endian),
//!   `/CIDCount`, `/GDBytes 2`, `/FontType 42`, `/FontMatrix` and `/PaintType`
//!   added — pdftops' exact set, the same identity CIDMap gs's own
//!   `.definettcidfont` builds — the block defines and renders ink through
//!   `composefont`. `/W` rides here (or `/DW 1000` when there is none); the
//!   widths never position anything under this device (xyshow supplies every
//!   displacement) but carry the PDF's metrics for consumers that re-use the
//!   font.
//! - **The parent is `composefont`**, not `definefont`: `/<font> /<cmap>
//!   [/<descendant>] composefont pop` builds the FMapType-9 composite gs's
//!   `zbuildfont0` requires and files it under the font's name for
//!   `findfont`. `definefont` on a PDF-shaped Type 0 dict fails
//!   `/invalidfont` (gs wants `/FMapType 9` + font-instance `/FDepVector` +
//!   the CMap as a dict).
//! - `/CMapType` is **1** (gs rangechecks on 2; 1 is what Adobe's CMaps and
//!   pdftops emit), and the CMap's hex operands are uppercase while font
//!   program payloads are lowercase ASCII ([`fonts::hex_payload`] style).
//!
//! A codespace note: the CMap maps *our assigned codes* to CIDs; for identity
//! runs code == CID so the mappings collapse, and gid 0 (whose code 0 the
//! 1-byte arms must avoid) is remapped to the highest unused code, where it
//! rides as an explicit cidchar (Task 5's `FontTable::cid_code`).

use crate::fonts::{hex_payload, ps_string};

/// `/CIDSystemInfo` carried from the PDF's descendant dict (or
/// `Adobe`/`Identity`/0 for promoted simple CFF fonts, where cid == gid).
///
/// Only the CIDFontType 2 emitter writes it — the ReadData form of
/// [`cidfont_type0`] has no home for it (gs derives it from the CFF).
#[derive(Clone, Debug)]
pub(crate) struct CidSystemInfo {
    pub registry: String,
    pub ordering: String,
    pub supplement: i32,
}

/// The `Adobe`/`Identity`/0 collection: what a promoted simple CFF's CIDs
/// belong to by construction (cid == gid), and the graceful default for a
/// composite whose PDF descendant carried no usable `/CIDSystemInfo`.
pub(crate) fn identity_csi() -> CidSystemInfo {
    CidSystemInfo { registry: "Adobe".into(), ordering: "Identity".into(), supplement: 0 }
}

/// How many bytes one hex-string chunk of an embedded font program carries:
/// PostScript strings top out at 65535 bytes, so a larger program must spill
/// into an array (the same chunk size the Type 42 sfnts use).
const CHUNK: usize = 65520;

/// How many cidrange/cidchar entries one `begin…` group holds (the batch size
/// the CMap grammar caps at 100; 12 keeps every group one screenful).
const GROUP: usize = 12;

/// A CMap resource mapping assigned codes to CIDs over a 1- or 2-byte code
/// space. `mappings` is `(code, cid)`; consecutive code == cid runs collapse
/// into cidranges, every other mapping rides as an explicit cidchar — the
/// `begincidrange`/`begincidchar` grammar, whose values are plain CID
/// integers (module docs: the bfchar/bfrange form maps nothing in gs).
///
/// Writes one complete `%%BeginResource: cmap <name>` block, the canonical
/// Adobe form: `/CIDInit /ProcSet findresource begin`, a 12 dict, the
/// `begincmap … endcmap` body — `/CMapName`, `/CMapType 1`, one
/// codespacerange (`<00> <FF>` for a 1-byte space, `<0000> <FFFF>` for
/// 2-byte), then the cidrange groups and the cidchar groups — closed by
/// `CMapName currentdict /CMap defineresource pop end end` (the name, the
/// dict and the category are `defineresource`'s exact three operands; the two
/// `end`s close the 12 dict and the ProcSet). `code_bytes` values other than
/// 2 are treated as 1; codes are never truncated, so a caller passing a code
/// wider than its declared space gets a wider hex string, not a wrong
/// mapping.
pub(crate) fn cmap_resource(out: &mut Vec<u8>, name: &str, code_bytes: u8, mappings: &[(u16, u32)]) {
    out.extend_from_slice(format!("%%BeginResource: cmap {name}\n").as_bytes());
    out.extend_from_slice(b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
    out.extend_from_slice(format!("/CMapName /{name} def\n").as_bytes());
    out.extend_from_slice(b"/CMapType 1 def\n");
    let (lo, hi) = if code_bytes == 2 { ("0000", "FFFF") } else { ("00", "FF") };
    out.extend_from_slice(format!("1 begincodespacerange\n<{lo}> <{hi}>\nendcodespacerange\n").as_bytes());
    let (ranges, chars) = split_mappings(mappings);
    for group in ranges.chunks(GROUP) {
        out.extend_from_slice(format!("{} begincidrange\n", group.len()).as_bytes());
        for &(lo, hi) in group {
            out.extend_from_slice(format!("{} {} {}\n", code_hex(lo, code_bytes), code_hex(hi, code_bytes), lo).as_bytes());
        }
        out.extend_from_slice(b"endcidrange\n");
    }
    for group in chars.chunks(GROUP) {
        out.extend_from_slice(format!("{} begincidchar\n", group.len()).as_bytes());
        for &(code, cid) in group {
            out.extend_from_slice(format!("{} {}\n", code_hex(u32::from(code), code_bytes), cid).as_bytes());
        }
        out.extend_from_slice(b"endcidchar\n");
    }
    out.extend_from_slice(b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n%%EndResource\n");
}

/// A CIDFontType0 descendant: the bare CFF program promoted by gs's own CFF
/// loader (the design spec's "FontFile3 bytes re-labelled as the CIDFontType0
/// program … near-passthrough").
///
/// Writes one complete `%%BeginResource: cidfont <name>` block:
///
/// ```text
/// /FontSetInit /ProcSet findresource begin
/// /<name> currentfile /ASCIIHexDecode filter //true //true ReadData
/// <the CFF, hex-encoded across the following lines>
/// >
/// pop
/// ```
///
/// ReadData parses the whole CFF from the currentfile filter and defines it
/// as a CIDFont resource under `<name>`: name-keyed CFFs are promoted with
/// CID == charstring index, CID-keyed ones keep their own charset's CIDs.
/// Because the resource is built entirely from the CFF, `/DW`, `/W` and a
/// supplied `/CIDSystemInfo` have **no home** in this form — the spec's
/// near-passthrough decision (module docs) — so this emitter takes neither:
/// glyph advances live in the CFF's own charstrings, and positioning never
/// consults widths anyway (the device's `xyshow` supplies every displacement).
/// `ReadData` is last on its line on purpose (the hex follows through the
/// filter); the `pop` drops the FontSet ReadData leaves. `begin` and
/// ReadData's internal `end`s balance; the `>` closes the hex for the filter.
pub(crate) fn cidfont_type0(out: &mut Vec<u8>, name: &str, cff: &[u8]) {
    out.extend_from_slice(format!("%%BeginResource: cidfont {name}\n").as_bytes());
    out.extend_from_slice(b"/FontSetInit /ProcSet findresource begin\n");
    // Last token on the line: everything after it is pulled through the
    // currentfile filter as data until the `>`.
    out.extend_from_slice(format!("/{name} currentfile /ASCIIHexDecode filter //true //true ReadData\n").as_bytes());
    out.extend_from_slice(hex_lines(cff).as_bytes());
    out.extend_from_slice(b">\npop\n%%EndResource\n");
}

/// A CIDFontType2 descendant embedding whole TrueType sfnts with an identity
/// CIDMap: the pinned PDF-transliterated keys plus gs's type-11 key set
/// (pdftops' combination — module docs).
///
/// Writes one complete `%%BeginResource: cidfont <name>` block whose dict
/// carries `/CIDFontName`, `/CIDFontType 2`, `/CIDSystemInfo`, the width
/// entries (`/W` with the array literal `w` holds, verbatim, or `/DW 1000` —
/// PDF's own default — when there is none), `/CIDToGIDMap /Identity` (CIDs
/// are gids, the statement our code assignment makes true), and the type-11
/// set gs's `.buildfont11` demands: `/FontType 42`, `/PaintType 0`,
/// `/FontMatrix [1 0 0 1 0 0]` (the device's `1000 scalefont` convention; gs
/// applies 1/unitsPerEm itself, as with Type 42), `/CIDMap` — a big-endian
/// 2-bytes-per-CID identity map over `num_glyphs` entries, chunked into an
/// array of ≤65535-byte strings when it cannot be one string (gs's own
/// `.definettcidfont` layout) — `/CIDCount` and `/GDBytes 2`. `/sfnts` holds
/// the whole TrueType program hex-encoded in [`hex_payload`] chunks. The
/// definition tail `CIDFontName currentdict end /CIDFont defineresource pop`
/// supplies its own key (the name, the dict, the category — `defineresource`'s
/// exact three operands), so the block opens `12 dict begin` with no leading
/// literal that would leak an operand.
pub(crate) fn cidfont_type2(out: &mut Vec<u8>, name: &str, sfnt: &[u8], num_glyphs: u16, csi: &CidSystemInfo, w: Option<&str>) {
    out.extend_from_slice(format!("%%BeginResource: cidfont {name}\n12 dict begin\n").as_bytes());
    out.extend_from_slice(format!("/CIDFontName /{name} def\n/CIDFontType 2 def\n").as_bytes());
    emit_csi(out, csi);
    emit_metrics(out, w);
    out.extend_from_slice(b"/CIDToGIDMap /Identity def\n");
    out.extend_from_slice(b"/FontType 42 def\n/PaintType 0 def\n/FontMatrix [1 0 0 1 0 0] def\n");
    emit_cid_map(out, num_glyphs);
    out.extend_from_slice(format!("/CIDCount {num_glyphs} def\n/GDBytes 2 def\n").as_bytes());
    out.extend_from_slice(b"/sfnts [\n");
    for chunk in sfnt.chunks(CHUNK) {
        out.extend_from_slice(hex_payload(chunk).as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"] def\n");
    out.extend_from_slice(b"CIDFontName currentdict end /CIDFont defineresource pop\n%%EndResource\n");
}

/// The Type 0 parent: `composefont` over the CMap and descendant by name.
///
/// Writes one complete `%%BeginResource: font <name>` block containing
/// exactly `/<name> /<cmap_name> [/<descendant_name>] composefont pop`:
/// `composefont` takes the font's name, the CMap (by name — it resolves
/// through `/CMap findresource`), and the FDepVector as an array of
/// descendant names (resolved through `/CIDFont findresource`), builds the
/// FMapType-9 composite gs's Type 0 machinery requires, and defines it under
/// `<name>` for `findfont`; the `pop` drops the font it returns. A plain
/// `definefont` on a PDF-shaped Type 0 dict is `/invalidfont` in gs (module
/// docs).
pub(crate) fn type0_parent(out: &mut Vec<u8>, name: &str, cmap_name: &str, descendant_name: &str) {
    out.extend_from_slice(format!("%%BeginResource: font {name}\n").as_bytes());
    out.extend_from_slice(format!("/{name} /{cmap_name} [/{descendant_name}] composefont pop\n%%EndResource\n").as_bytes());
}

/// A mapping set split for emission: the identity cidrange runs (as
/// `(lo, hi)` code spans, each mapping a code to itself) and the remaining
/// explicit cidchar entries `(code, cid)`.
type MappingSplit = (Vec<(u32, u32)>, Vec<(u16, u32)>);

/// Split `mappings` (sorted by code, deterministically) into identity
/// cidrange runs — maximal sequences of consecutive codes where every code
/// equals its CID — and the remaining explicit cidchar entries. All run
/// arithmetic is over `u32` with checked adds.
fn split_mappings(mappings: &[(u16, u32)]) -> MappingSplit {
    let mut sorted = mappings.to_vec();
    sorted.sort_unstable();
    let mut ranges = Vec::new();
    let mut chars = Vec::new();
    let mut i = 0;
    while let Some(&(code, cid)) = sorted.get(i) {
        if u32::from(code) == cid {
            let mut hi = cid;
            let mut j = i + 1;
            while let Some(&(next_code, next_cid)) = sorted.get(j) {
                let Some(next) = hi.checked_add(1) else { break };
                if next_cid == next && u32::from(next_code) == next_cid {
                    hi = next_cid;
                    j += 1;
                } else {
                    break;
                }
            }
            ranges.push((cid, hi));
            i = j;
        } else {
            chars.push((code, cid));
            i += 1;
        }
    }
    (ranges, chars)
}

/// `/CIDSystemInfo` in the 3-dict form every CID emitter shares: Registry and
/// Ordering as PostScript strings (escaped by [`ps_string`] — they come from
/// the PDF, whose strings are not bound by name rules), Supplement as the
/// integer it is.
fn emit_csi(out: &mut Vec<u8>, csi: &CidSystemInfo) {
    out.extend_from_slice(b"/CIDSystemInfo 3 dict dup begin\n");
    out.extend_from_slice(format!("/Registry {} def\n", ps_string(csi.registry.as_bytes())).as_bytes());
    out.extend_from_slice(format!("/Ordering {} def\n", ps_string(csi.ordering.as_bytes())).as_bytes());
    out.extend_from_slice(format!("/Supplement {} def\n", csi.supplement).as_bytes());
    out.extend_from_slice(b"end def\n");
}

/// The width entries: `/W` with the array literal `w` holds, verbatim
/// (the strategy re-serialized the PDF's `/W` into it — numbers and nested
/// arrays only, brackets included), or `/DW 1000` when there is none — PDF's
/// own default, an honest em width when the font says nothing.
fn emit_metrics(out: &mut Vec<u8>, w: Option<&str>) {
    match w {
        Some(w) => out.extend_from_slice(format!("/W {w} def\n").as_bytes()),
        None => out.extend_from_slice(b"/DW 1000 def\n"),
    }
}

/// The identity `/CIDMap` over `num_glyphs` entries: 2 big-endian bytes per
/// CID (CID i → glyph i), one string while it fits the 65535-byte string
/// limit, an array of strings beyond — gs's `.definettcidfont` layout, which
/// its `.buildfont11` string-array access accepts for both shapes. The hex
/// wraps near 200 columns (whitespace is free inside a hex string).
fn emit_cid_map(out: &mut Vec<u8>, num_glyphs: u16) {
    let entries = usize::from(num_glyphs);
    // 32760 entries = 65520 bytes = the CHUNK the sfnts already use.
    const PER_STRING: usize = 32760;
    let chunks = entries.div_ceil(PER_STRING).max(1);
    let emit_chunk = |out: &mut Vec<u8>, lo: usize, hi: usize| {
        out.push(b'<');
        let mut column = 1usize;
        for cid in lo..hi {
            // The value is the CID itself; a >0xFFFF entry cannot exist (u16
            // count), and 4 hex digits always carry it.
            let entry = format!("{cid:04x}");
            if column + entry.len() > 200 {
                out.push(b'\n');
                column = 0;
            }
            out.extend_from_slice(entry.as_bytes());
            column += entry.len();
        }
        out.push(b'>');
    };
    if chunks == 1 {
        out.extend_from_slice(b"/CIDMap ");
        emit_chunk(out, 0, entries);
        out.extend_from_slice(b" def\n");
    } else {
        out.extend_from_slice(b"/CIDMap [\n");
        for c in 0..chunks {
            let lo = c * PER_STRING;
            let hi = (lo + PER_STRING).min(entries);
            emit_chunk(out, lo, hi);
            out.push(b'\n');
        }
        out.extend_from_slice(b"] def\n");
    }
}

/// `bytes` as bare lowercase hex wrapped near 200 columns, for the
/// `currentfile /ASCIIHexDecode filter` route: the filter reads the following
/// program text as one hex stream (whitespace is free), so no `<…>` string
/// literal is involved and no 65535-byte string limit applies — the caller's
/// `>` terminator closes it.
fn hex_lines(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2).saturating_add(8));
    let mut column = 0usize;
    for byte in bytes {
        if column + 2 > 200 {
            out.push('\n');
            column = 0;
        }
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
        column += 2;
    }
    out.push('\n');
    out
}

/// A code as a `<…>` hex string of the code space's width — 2 digits per
/// byte, uppercase like the repo's existing CMap strings. The width is a
/// minimum, so a code wider than its declared space prints wider rather than
/// mapping to something wrong.
fn code_hex(code: u32, code_bytes: u8) -> String {
    let digits = usize::from(code_bytes.clamp(1, 2)) * 2;
    format!("<{code:0digits$X}>")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------- fixtures

    /// A minimal CFF (Type1C) font: `.notdef`, `A` (one curve) and `B` (a box).
    /// Copied from `crates/ps-export/src/fonts.rs`'s test module (house style:
    /// each test module owns its fixtures).
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

    /// A stand-in TrueType program: these tests assert emission bytes, not
    /// outlines, so the payload only needs a recognizable sfnt magic.
    const SFNT: [u8; 12] = [0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe];

    /// The CIDSystemInfo the tests carry: a non-Identity ordering, so the
    /// assertions prove the values travel from the argument into the dict.
    fn csi() -> CidSystemInfo {
        CidSystemInfo { registry: "Adobe".into(), ordering: "Japan1".into(), supplement: 2 }
    }

    /// An emitter's output as a string (the emission is pure ASCII).
    fn text(out: &[u8]) -> String {
        String::from_utf8(out.to_vec()).expect("ASCII output")
    }

    // ------------------------------------------------------------------ tests

    /// Consecutive identity mappings collapse into cidranges over a 2-byte
    /// code space: `[(3,3),(4,4)]` is one run, `(9,9)` another, and the
    /// codespace is `<0000> <FFFF>`. Values are plain CID integers — the
    /// grammar gs maps codes through (module docs).
    #[test]
    fn identity_mappings_collapse_to_cidranges() {
        let mut out = Vec::new();
        cmap_resource(&mut out, "TestCMap", 2, &[(3, 3), (4, 4), (9, 9)]);
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cmap TestCMap\n"), "one complete resource block: {s}");
        assert!(s.contains("begincmap\n"), "the standard CMap body: {s}");
        assert!(s.contains("/CMapName /TestCMap def\n"), "the CMap's name: {s}");
        assert!(s.contains("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"), "2-byte codespace: {s}");
        assert!(s.contains("<0003> <0004> 3\n"), "the (3,3),(4,4) run as one cidrange: {s}");
        assert!(s.contains("<0009> <0009> 9\n"), "the lone identity mapping as a cidrange: {s}");
        assert!(s.contains("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"), "canonical resource tail: {s}");
        assert!(!s.contains("cidchar"), "identity mappings never need cidchars: {s}");
    }

    /// A mapping whose code is not its CID — the remapped gid-0 entry, which
    /// the strategy assigns the highest unused code — rides as an explicit
    /// cidchar while identity mappings keep their cidranges.
    #[test]
    fn non_identity_mappings_use_explicit_cidchars() {
        let mut out = Vec::new();
        cmap_resource(&mut out, "TestCMap", 2, &[(1, 1), (0xFFFF, 0)]);
        let s = text(&out);
        assert!(s.contains("1 begincidchar\n<FFFF> 0\nendcidchar\n"), "the remapped gid-0 entry as a cidchar line: {s}");
        assert!(s.contains("<0001> <0001> 1\n"), "the identity mapping stays a cidrange: {s}");
        assert!(s.contains("%%EndResource\n"), "the block closes: {s}");
    }

    /// The CIDFontType 0 descendant ships the CFF through gs's own loader:
    /// the FontSetInit prologue, the ReadData line with the hex following on
    /// the next lines, the `>` terminator and the `pop`. No `/W`, `/DW` or
    /// `/CIDSystemInfo` — the resource is built from the CFF itself (the
    /// near-passthrough decision, module docs).
    #[test]
    fn cidfont_type0_loads_via_readdata() {
        let cff = cff_program();
        let mut out = Vec::new();
        cidfont_type0(&mut out, "TestD0", &cff);
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cidfont TestD0\n"), "one complete resource block: {s}");
        assert!(s.contains("/FontSetInit /ProcSet findresource begin\n"), "the loader prologue: {s}");
        assert!(
            s.contains("/TestD0 currentfile /ASCIIHexDecode filter //true //true ReadData\n"),
            "the promotion line, ReadData last so the following hex is the data: {s}"
        );
        // The hex of the whole CFF rides between the ReadData line and the
        // `>` — the first bytes of the program verbatim.
        let hex_head: String = cff.iter().take(16).map(|b| format!("{b:02x}")).collect();
        assert!(s.contains(&hex_head), "the CFF bytes as hex: {s}");
        assert!(s.ends_with(">\npop\n%%EndResource\n"), "the `>` closes the hex, the pop drops the FontSet: {s}");
        assert!(!s.contains("/W ") && !s.contains("/DW"), "no metrics keys: the near-passthrough form has no home for them: {s}");
        assert!(!s.contains("CIDSystemInfo"), "gs derives the CIDSystemInfo from the CFF: {s}");
    }

    /// The CIDFontType 2 descendant declares `/CIDToGIDMap /Identity` (CIDs
    /// are gids), carries gs's type-11 key set — the identity `/CIDMap`
    /// string, `/CIDCount`, `/GDBytes 2`, `/FontType 42`, `/FontMatrix`,
    /// `/PaintType` — embeds the whole TrueType program in `/sfnts` hex, and
    /// carries the `/W` array verbatim when the PDF gave one.
    #[test]
    fn cidfont_type2_declares_identity_cidtogidmap() {
        let mut out = Vec::new();
        cidfont_type2(&mut out, "TestD1", &SFNT, 3, &csi(), Some("[1 [500 700] 3 250]"));
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cidfont TestD1\n"), "one complete resource block: {s}");
        assert!(s.contains("/CIDFontType 2 def\n"), "a CIDFontType 2 dict: {s}");
        assert!(s.contains("/CIDToGIDMap /Identity def\n"), "identity CID-to-GID mapping: {s}");
        assert!(s.contains("/CIDMap <000000010002> def\n"), "the identity CIDMap over 3 glyphs: {s}");
        assert!(s.contains("/CIDCount 3 def\n"), "the glyph count: {s}");
        assert!(s.contains("/GDBytes 2 def\n"), "2 bytes per glyph id: {s}");
        assert!(s.contains("/FontType 42 def\n"), "gs's type-11 shape: {s}");
        assert!(s.contains("/PaintType 0 def\n"), "gs's type-11 shape: {s}");
        assert!(s.contains("/FontMatrix [1 0 0 1 0 0] def\n"), "the 1000-scalefont convention: {s}");
        assert!(s.contains("/sfnts [\n"), "the sfnts entry: {s}");
        assert!(s.contains(&hex_payload(&SFNT)), "the whole TrueType program, hex-encoded: {s}");
        assert!(s.contains("/W [1 [500 700] 3 250] def\n"), "the /W array carried verbatim: {s}");
        assert!(!s.contains("/DW"), "an explicit /W needs no /DW: {s}");
        assert!(s.contains("CIDFontName currentdict end /CIDFont defineresource pop\n%%EndResource\n"), "resource definition tail: {s}");
    }

    /// A font with more glyphs than one CIDMap string can hold chunks the map
    /// into an array of strings (gs's `.definettcidfont` layout).
    #[test]
    fn cidfont_type2_chunks_big_cidmaps() {
        let mut out = Vec::new();
        cidfont_type2(&mut out, "TestD2", &SFNT, 32_800, &csi(), None);
        let s = text(&out);
        assert!(s.contains("/CIDMap [\n"), "an array of strings: {s}");
        assert!(s.contains("/DW 1000 def\n"), "no /W, the default width: {s}");
        // 32760 entries in the first string, 40 in the second; the last
        // entry of the first chunk is CID 32759 = 0x7ff7.
        assert!(s.contains("<000000010002"), "the map starts at identity: {s}");
        assert!(s.contains("7ff7>\n"), "the first chunk ends at CID 32759: {s}");
        assert!(s.contains("/CIDCount 32800 def\n"), "the full glyph count: {s}");
    }

    /// The Type 0 parent ties the CMap and the descendant by name through
    /// `composefont` — gs's route to a findfont-able composite.
    #[test]
    fn type0_parent_ties_cmap_and_descendant() {
        let mut out = Vec::new();
        type0_parent(&mut out, "TestF0", "TestCMap", "TestD0");
        let s = text(&out);
        assert!(s.contains("%%BeginResource: font TestF0\n"), "one complete resource block: {s}");
        assert!(s.contains("/TestF0 /TestCMap [/TestD0] composefont pop\n"), "composefont over the names, its result dropped: {s}");
        assert!(s.ends_with("%%EndResource\n"), "the block closes: {s}");
    }
}
