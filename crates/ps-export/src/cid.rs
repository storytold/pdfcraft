//! CID-keyed emission: identity CMaps and CIDFont dictionaries for the
//! composite arms (Layer B Task 4).
//!
//! CFF and CID fonts re-emit as PostScript **Type 0 composites**: a CMap
//! resource mapping our assigned codes to the font's CIDs, a CIDFont
//! descendant resource embedding the font program, and a Type 0 parent tying
//! them together. A font's blocks go into the preamble in that order — CMap,
//! descendant, parent — the design's ordering rule (Task 5 wires the calls).
//!
//! Emission shape: the CMap is the canonical Adobe resource form
//! (`/CIDInit /ProcSet findresource begin`, `begincmap … endcmap`, closed by
//! `CMapName currentdict /CMap defineresource pop end end` — the tail of every
//! CMap Adobe ships and of pdftops' output alike, with the name, the dict and
//! the category as `defineresource`'s exact three operands); the CIDFont
//! resources end `CIDFontName currentdict end /CIDFont defineresource pop`
//! (same three operands, the `end` coming between `currentdict` and the
//! category so the dict leaves the dictionary stack while staying on the
//! operand stack); the parent ends `currentdict end definefont pop` — the
//! name and the dict are exactly `definefont`'s two operands, the discipline
//! the Type 42 work fixed (a stray third operand silently broke `findfont`).
//!
//! Two shape notes from observing this repo's oracle, gs 10.08.0 (black-box,
//! synthetic probes): `/CMapType` is **1**, not the 2 of the PDF-side ToUnicode
//! fixtures — gs's `.buildcmap` rangechecks on 2, and 1 is what Adobe's own
//! CMaps and pdftops emit; and the hex operands of the CMap body are
//! uppercase (the style of the repo's existing CMap strings) while the font
//! program payloads reuse [`fonts::hex_payload`] and its lowercase ASCII.
//! Execution arbitration belongs to Task 6's gs gates; these emitters assert
//! structure and bytes only.

use crate::fonts::{hex_payload, ps_string};

/// `/CIDSystemInfo` carried from the PDF's descendant dict (or
/// `Adobe`/`Identity`/0 for promoted simple CFF fonts, where cid == gid).
#[derive(Clone, Debug)]
pub(crate) struct CidSystemInfo {
    pub registry: String,
    pub ordering: String,
    pub supplement: i32,
}

/// How many bytes one hex-string chunk of an embedded font program carries:
/// PostScript strings top out at 65535 bytes, so a larger program must spill
/// into an array (the same chunk size the Type 42 sfnts use).
const CHUNK: usize = 65520;

/// How many bfrange/bfchar entries one `begin…` group holds (the batch size
/// the CMap grammar caps at 100; 12 keeps every group one screenful).
const GROUP: usize = 12;

/// A CMap resource mapping assigned codes to CIDs over a 1- or 2-byte code
/// space. `mappings` is `(code, cid)`; consecutive code == cid runs collapse
/// into identity bfranges, every other mapping rides as an explicit bfchar.
///
/// Writes one complete `%%BeginResource: cmap <name>` block, the canonical
/// Adobe form: `/CIDInit /ProcSet findresource begin`, a 12 dict, the
/// `begincmap … endcmap` body — `/CMapName`, `/CMapType 1`, one codespacerange
/// (`<00> <FF>` for a 1-byte space, `<0000> <FFFF>` for 2-byte), then the
/// bfrange groups and the bfchar groups, at most [`GROUP`] entries per group —
/// closed by `CMapName currentdict /CMap defineresource pop end end` (the
/// name, the dict and the category are `defineresource`'s exact three
/// operands; the two `end`s close the 12 dict and the ProcSet). `code_bytes`
/// values other than 2 are treated as 1; codes are never truncated, so a
/// caller passing a code wider than its declared space gets a wider hex
/// string, not a wrong mapping.
pub(crate) fn cmap_resource(out: &mut Vec<u8>, name: &str, code_bytes: u8, mappings: &[(u16, u32)]) {
    out.extend_from_slice(format!("%%BeginResource: cmap {name}\n").as_bytes());
    out.extend_from_slice(b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
    out.extend_from_slice(format!("/CMapName /{name} def\n").as_bytes());
    out.extend_from_slice(b"/CMapType 1 def\n");
    let (lo, hi) = if code_bytes == 2 { ("0000", "FFFF") } else { ("00", "FF") };
    out.extend_from_slice(format!("1 begincodespacerange\n<{lo}> <{hi}>\nendcodespacerange\n").as_bytes());
    let (ranges, chars) = split_mappings(mappings);
    for group in ranges.chunks(GROUP) {
        out.extend_from_slice(format!("{} beginbfrange\n", group.len()).as_bytes());
        for &(lo, hi) in group {
            out.extend_from_slice(format!("{} {} {}\n", code_hex(lo, code_bytes), code_hex(hi, code_bytes), cid_hex(lo)).as_bytes());
        }
        out.extend_from_slice(b"endbfrange\n");
    }
    for group in chars.chunks(GROUP) {
        out.extend_from_slice(format!("{} beginbfchar\n", group.len()).as_bytes());
        for &(code, cid) in group {
            out.extend_from_slice(format!("{} {}\n", code_hex(u32::from(code), code_bytes), cid_hex(cid)).as_bytes());
        }
        out.extend_from_slice(b"endbfchar\n");
    }
    out.extend_from_slice(b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n%%EndResource\n");
}

/// A CIDFontType0 descendant embedding bare CFF bytes (FontFile3-style hex).
///
/// Writes one complete `%%BeginResource: cidfont <name>` block whose dict
/// carries `/CIDFontName`, `/CIDFontType 0`, `/CIDSystemInfo` (the 3-dict
/// Registry/Ordering/Supplement form pdftops and Adobe's CIDFonts write),
/// the width entries (`/W` with the array literal `w` holds, verbatim, or
/// `/DW 1000` — PDF's own default — when there is none) and `/GlyphData`:
/// the CFF program hex-encoded in [`hex_payload`] chunks, the PostScript
/// counterpart of PDF's `/FontFile3` stream. The block opens `12 dict
/// begin` with no leading literal (unlike the Type 42/parent `definefont`
/// shape, whose key operand must ride the stack): the definition tail
/// `CIDFontName currentdict end /CIDFont defineresource pop` supplies its
/// own key — the name, the dict and the category as `defineresource`'s
/// three operands, the `end` between `currentdict` and the category
/// popping the dictionary stack while the dict stays on the operand stack —
/// so a leading literal would leak one operand.
pub(crate) fn cidfont_type0(out: &mut Vec<u8>, name: &str, cff: &[u8], csi: &CidSystemInfo, w: Option<&str>) {
    out.extend_from_slice(format!("%%BeginResource: cidfont {name}\n12 dict begin\n").as_bytes());
    out.extend_from_slice(format!("/CIDFontName /{name} def\n/CIDFontType 0 def\n").as_bytes());
    emit_csi(out, csi);
    emit_metrics(out, w);
    out.extend_from_slice(b"/GlyphData [\n");
    for chunk in cff.chunks(CHUNK) {
        out.extend_from_slice(hex_payload(chunk).as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"] def\n");
    out.extend_from_slice(b"CIDFontName currentdict end /CIDFont defineresource pop\n%%EndResource\n");
}

/// A CIDFontType2 descendant embedding whole TrueType sfnts, /CIDToGIDMap /Identity.
///
/// Writes one complete `%%BeginResource: cidfont <name>` block whose dict
/// carries `/CIDFontName`, `/CIDFontType 2`, `/CIDSystemInfo`, the width
/// entries (`/W` verbatim or `/DW 1000`, as [`cidfont_type0`]),
/// `/CIDToGIDMap /Identity` (CIDs are gids — the same statement PDF's
/// Identity value makes) and `/sfnts`: the whole TrueType program
/// hex-encoded in [`hex_payload`] chunks, chunked like the Type 42 sfnts
/// because PostScript strings top out at 65535 bytes. The block's opening
/// and definition tail match [`cidfont_type0`].
pub(crate) fn cidfont_type2(out: &mut Vec<u8>, name: &str, sfnt: &[u8], csi: &CidSystemInfo, w: Option<&str>) {
    out.extend_from_slice(format!("%%BeginResource: cidfont {name}\n12 dict begin\n").as_bytes());
    out.extend_from_slice(format!("/CIDFontName /{name} def\n/CIDFontType 2 def\n").as_bytes());
    emit_csi(out, csi);
    emit_metrics(out, w);
    out.extend_from_slice(b"/CIDToGIDMap /Identity def\n");
    out.extend_from_slice(b"/sfnts [\n");
    for chunk in sfnt.chunks(CHUNK) {
        out.extend_from_slice(hex_payload(chunk).as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"] def\n");
    out.extend_from_slice(b"CIDFontName currentdict end /CIDFont defineresource pop\n%%EndResource\n");
}

/// The Type 0 parent: /FontType 0 /CMap <cmap> /CIDFont <descendant>.
///
/// Writes one complete `%%BeginResource: font <name>` block whose dict
/// carries `/FontName`, `/FontType 0`, `/FontMatrix` (the identity — the
/// device's `1000 scalefont` convention scales the composite like every
/// other arm) and the two tie entries `/CMap /<cmap_name>` and
/// `/CIDFont /<descendant_name>`, both by name. The definition ends
/// `currentdict end definefont pop`: the leading `/<name> N dict begin`
/// keeps the name on the operand stack through the body, so `definefont`
/// receives exactly its two operands (a stray third is the bug that once
/// left the Type 42 dict filed under the wrong FontDirectory key,
/// invisible to `findfont`).
pub(crate) fn type0_parent(out: &mut Vec<u8>, name: &str, cmap_name: &str, descendant_name: &str) {
    out.extend_from_slice(format!("%%BeginResource: font {name}\n/{name} 8 dict begin\n").as_bytes());
    out.extend_from_slice(format!("/FontName /{name} def\n/FontType 0 def\n/FontMatrix [1 0 0 1 0 0] def\n").as_bytes());
    out.extend_from_slice(format!("/CMap /{cmap_name} def\n/CIDFont /{descendant_name} def\n").as_bytes());
    out.extend_from_slice(b"currentdict end definefont pop\n%%EndResource\n");
}

/// A mapping set split for emission: the identity bfrange runs (as `(lo, hi)`
/// code spans, each mapping a code to itself) and the remaining explicit
/// bfchar entries `(code, cid)`.
type MappingSplit = (Vec<(u32, u32)>, Vec<(u16, u32)>);

/// Split `mappings` (sorted by code, deterministically) into identity
/// bfrange runs — maximal sequences of consecutive codes where every code
/// equals its CID — and the remaining explicit bfchar entries. All run
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
/// (Task 5 re-serializes the PDF's `/W` into it — numbers and nested arrays
/// only, brackets included), or `/DW 1000` when there is none — PDF's own
/// default, an honest em width when the font says nothing.
fn emit_metrics(out: &mut Vec<u8>, w: Option<&str>) {
    match w {
        Some(w) => out.extend_from_slice(format!("/W {w} def\n").as_bytes()),
        None => out.extend_from_slice(b"/DW 1000 def\n"),
    }
}

/// A code as a `<…>` hex string of the code space's width — 2 digits per
/// byte, uppercase like the repo's existing CMap strings. The width is a
/// minimum, so a code wider than its declared space prints wider rather
/// than mapping to something wrong.
fn code_hex(code: u32, code_bytes: u8) -> String {
    let digits = usize::from(code_bytes.clamp(1, 2)) * 2;
    format!("<{code:0digits$X}>")
}

/// A CID as a `<…>` hex string: 2 bytes (4 digits) — the CMap grammar's
/// usual width — widening to 4 bytes when the CID cannot fit 16 bits.
fn cid_hex(cid: u32) -> String {
    if cid <= 0xFFFF { format!("<{cid:04X}>") } else { format!("<{cid:08X}>") }
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

    /// Consecutive identity mappings collapse into bfranges over a 2-byte code
    /// space: `[(3,3),(4,4)]` is one run, `(9,9)` another, and the codespace
    /// is `<0000> <FFFF>`.
    #[test]
    fn identity_mappings_collapse_to_bfranges() {
        let mut out = Vec::new();
        cmap_resource(&mut out, "TestCMap", 2, &[(3, 3), (4, 4), (9, 9)]);
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cmap TestCMap\n"), "one complete resource block: {s}");
        assert!(s.contains("begincmap\n"), "the standard CMap body: {s}");
        assert!(s.contains("/CMapName /TestCMap def\n"), "the CMap's name: {s}");
        assert!(s.contains("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"), "2-byte codespace: {s}");
        assert!(s.contains("<0003> <0004> <0003>\n"), "the (3,3),(4,4) run as one bfrange: {s}");
        assert!(s.contains("<0009> <0009> <0009>\n"), "the lone identity mapping as a bfrange: {s}");
        assert!(s.contains("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"), "canonical resource tail: {s}");
        assert!(!s.contains("bfchar"), "identity mappings never need bfchars: {s}");
    }

    /// A mapping whose code is not its CID — the remapped gid-0 entry, which
    /// Task 5 assigns the highest unused code — rides as an explicit bfchar
    /// while identity mappings keep their bfranges.
    #[test]
    fn non_identity_mappings_use_explicit_bfchars() {
        let mut out = Vec::new();
        cmap_resource(&mut out, "TestCMap", 2, &[(1, 1), (0xFFFF, 0)]);
        let s = text(&out);
        assert!(s.contains("1 beginbfchar\n<FFFF> <0000>\nendbfchar\n"), "the remapped gid-0 entry as a bfchar line: {s}");
        assert!(s.contains("<0001> <0001> <0001>\n"), "the identity mapping stays a bfrange: {s}");
        assert!(s.contains("%%EndResource\n"), "the block closes: {s}");
    }

    /// The CIDFontType 0 descendant embeds the bare CFF program hex-encoded
    /// (the `/FontFile3`-equivalent `/GlyphData` string) and carries
    /// `/CIDFontType 0`, the given CIDSystemInfo and `/DW 1000` when the PDF's
    /// `/W` could not be carried.
    #[test]
    fn cidfont_type0_embeds_cff_bytes() {
        let cff = cff_program();
        let mut out = Vec::new();
        cidfont_type0(&mut out, "TestD0", &cff, &csi(), None);
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cidfont TestD0\n"), "one complete resource block: {s}");
        assert!(s.contains("/CIDFontType 0 def\n"), "a CIDFontType 0 dict: {s}");
        assert!(s.contains("/Registry (Adobe) def\n"), "the given registry: {s}");
        assert!(s.contains("/Ordering (Japan1) def\n"), "the given ordering: {s}");
        assert!(s.contains("/Supplement 2 def\n"), "the given supplement: {s}");
        assert!(s.contains("/DW 1000 def\n"), "the default width when /W is absent: {s}");
        assert!(s.contains("/GlyphData [\n"), "the FontFile3-equivalent entry: {s}");
        assert!(s.contains(&hex_payload(&cff)), "the CFF bytes, hex-encoded: {s}");
        assert!(s.contains("CIDFontName currentdict end /CIDFont defineresource pop\n"), "resource definition tail: {s}");
        assert!(s.ends_with("%%EndResource\n"), "the block closes: {s}");
    }

    /// The CIDFontType 2 descendant declares `/CIDToGIDMap /Identity` (CIDs are
    /// gids), embeds the whole TrueType program in `/sfnts` hex, and carries
    /// the `/W` array verbatim when the PDF gave one.
    #[test]
    fn cidfont_type2_declares_identity_cidtogidmap() {
        let mut out = Vec::new();
        cidfont_type2(&mut out, "TestD1", &SFNT, &csi(), Some("[1 [500 700] 3 250]"));
        let s = text(&out);
        assert!(s.contains("%%BeginResource: cidfont TestD1\n"), "one complete resource block: {s}");
        assert!(s.contains("/CIDFontType 2 def\n"), "a CIDFontType 2 dict: {s}");
        assert!(s.contains("/CIDToGIDMap /Identity def\n"), "identity CID-to-GID mapping: {s}");
        assert!(s.contains("/sfnts [\n"), "the sfnts entry: {s}");
        assert!(s.contains(&hex_payload(&SFNT)), "the whole TrueType program, hex-encoded: {s}");
        assert!(s.contains("/W [1 [500 700] 3 250] def\n"), "the /W array carried verbatim: {s}");
        assert!(!s.contains("/DW"), "an explicit /W needs no /DW: {s}");
        assert!(s.contains("CIDFontName currentdict end /CIDFont defineresource pop\n%%EndResource\n"), "resource definition tail: {s}");
    }

    /// The Type 0 parent ties the CMap and the descendant by name:
    /// `/FontType 0`, `/CMap /<cmap>`, `/CIDFont /<descendant>`, defined
    /// through `definefont`.
    #[test]
    fn type0_parent_ties_cmap_and_descendant() {
        let mut out = Vec::new();
        type0_parent(&mut out, "TestF0", "TestCMap", "TestD0");
        let s = text(&out);
        assert!(s.contains("%%BeginResource: font TestF0\n"), "one complete resource block: {s}");
        assert!(s.contains("/FontType 0 def\n"), "a Type 0 parent: {s}");
        assert!(s.contains("/CMap /TestCMap def\n"), "the CMap by name: {s}");
        assert!(s.contains("/CIDFont /TestD0 def\n"), "the descendant by name: {s}");
        assert!(s.contains("currentdict end definefont pop\n%%EndResource\n"), "definefont with exactly its two operands: {s}");
    }
}
