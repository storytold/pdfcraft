//! sfnt table-directory rewriting: replace or add a `cmap` so assigned char
//! codes resolve through the font's own tables, and rebuild the directory
//! around it.
//!
//! [`synthesize_cmap`] is the Layer B entry: the Type 42 emitter (Task 2)
//! assigns each used glyph a char code and rewrites the embedded sfnt so that
//! code maps to that glyph, the way mupdf builds symbol Type 42 fonts. The new
//! `cmap` leads with a (3,0) format-4 subtable built from maximal runs of
//! consecutive code→gid pairs plus the mandatory final `0xFFFF` segment, and
//! keeps every original subtable except a superseded (3,0) one alongside it,
//! so a consumer's gid→Unicode reverse mapping survives. Task 3's Ghostscript
//! gate arbitrates this choice: if ink or extraction fails there, the
//! documented fallbacks to try, in order, are a (1,0) format-6 and a (3,10)
//! format-12 subtable — and the working combination gets recorded here.
//!
//! Parsing goes through skrifa — the same version hayro-interpret uses, so
//! glyph ids agree by construction — while all writing is hand-rolled and
//! shared: [`write_directory`] and the checksum helpers rebuild the 12-byte
//! header, the 16-byte records in ascending-tag order, per-table checksums,
//! 4-byte alignment and the `head` `checkSumAdjustment`. Task 7's glyf/loca
//! subsetting reuses those internals for its own surgery. Every offset or
//! length read from the input is bounds-checked against the input's own
//! length: a malformed font answers `None` so the caller can degrade, never a
//! panic.

use std::collections::BTreeMap;

use skrifa::FontRef;
use skrifa::raw::TableProvider as _;

/// The value every TrueType whole-font checksum adds up to once
/// `head.checkSumAdjustment` holds the right number.
const CHECKSUM_MAGIC: u32 = 0xB1B0_AFBA;

/// Replace/extend an sfnt's cmap so `mappings` (char code → glyph id) resolve
/// through the font's own tables, rebuilding the table directory, checksums
/// and 4-byte alignment. `None` on any parse or bounds failure (caller degrades).
///
/// Mappings are deduplicated by code, the last one winning; a mapping from
/// code `0xFFFF` (which format 4 reserves for its final segment), to gid 0, or
/// to a glyph id at or past `maxp`'s `numGlyphs` cannot resolve and is
/// dropped. All other tables are carried over byte for byte, so a collection
/// input yields a standalone font built from face 0's tables.
pub(crate) fn synthesize_cmap(sfnt: &[u8], mappings: &[(u16, u16)]) -> Option<Vec<u8>> {
    let font = FontRef::from_index(sfnt, 0).ok()?;
    // Bounds the glyph ids below and guarantees a head table that can hold the
    // checkSumAdjustment; a font missing either is not one we can rewrite.
    let num_glyphs = font.maxp().ok()?.num_glyphs();
    font.head().ok()?;
    let mut old_cmap = None;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for record in font.table_directory().table_records() {
        let tag = record.tag().into_bytes();
        // `table_data` slices with the record's checked offset and length, so
        // a table claiming bytes past the end of the file answers `None` here.
        let data = font.table_data(record.tag())?.as_bytes().to_vec();
        if tag == *b"cmap" {
            old_cmap = Some(data); // replaced below, its subtables kept
        } else {
            tables.push((tag, data));
        }
    }
    // A hostile directory may repeat a tag; keep the first, drop the rest.
    tables.sort_by_key(|table| table.0);
    tables.dedup_by(|a, b| a.0 == b.0);
    tables.push((*b"cmap", synthesized_cmap_table(old_cmap.as_deref(), mappings, num_glyphs)?));
    write_directory(font.table_directory().sfnt_version(), &mut tables)
}

/// Build the replacement `cmap` table: a (3,0) format-4 symbol subtable for
/// `mappings` first, then every subtable of the old cmap except a superseded
/// (3,0) one — the reverse mapping consumers extract from the original
/// subtables survives the rewrite.
fn synthesized_cmap_table(old: Option<&[u8]>, mappings: &[(u16, u16)], num_glyphs: u16) -> Option<Vec<u8>> {
    let mut map = BTreeMap::new();
    for &(code, gid) in mappings {
        if code != 0xFFFF && gid != 0 && gid < num_glyphs {
            map.insert(code, gid);
        }
    }
    // Maximal runs where gid(code+1) == gid(code)+1 become one segment
    // `startCode..endCode` each, with idDelta = gid − startCode (mod 2¹⁶) and
    // idRangeOffset 0 — mupdf's construction — plus the mandatory final
    // 0xFFFF segment (idDelta 1, resolving 0xFFFF to .notdef).
    let mut segments: Vec<(u16, u16, u16)> = Vec::new(); // (startCode, endCode, idDelta)
    let mut entries = map.iter().peekable();
    while let Some((&code, &gid)) = entries.next() {
        let (mut end, mut last) = (code, gid);
        while let Some(&(&next_code, &next_gid)) = entries.peek() {
            if next_code == end.wrapping_add(1) && next_gid == last.wrapping_add(1) {
                end = next_code;
                last = next_gid;
                entries.next();
            } else {
                break;
            }
        }
        segments.push((code, end, gid.wrapping_sub(code)));
    }
    segments.push((0xFFFF, 0xFFFF, 1));
    let symbol = format4_subtable(&segments)?;

    // The subtables to keep, as (platformID, encodingID, bytes); an extent
    // outside the old table's own length fails the whole rewrite.
    let mut kept: Vec<(u16, u16, &[u8])> = Vec::new();
    if let Some(old) = old {
        let count = usize::from(be16(old, 2)?);
        for i in 0..count {
            let at = 4 + i * 8;
            let (platform, encoding) = (be16(old, at)?, be16(old, at.checked_add(2)?)?);
            let offset = usize::try_from(be32(old, at.checked_add(4)?)?).ok()?;
            if platform == 3 && encoding == 0 {
                continue; // superseded by the synthesized symbol subtable
            }
            let end = subtable_end(old, offset)?;
            kept.push((platform, encoding, old.get(offset..end)?));
        }
    }

    let count = 1 + kept.len();
    let subtables_at = 4 + count * 8; // cmap header + records, where subtables start
    let mut out = Vec::with_capacity(subtables_at + symbol.len());
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&u16::try_from(count).ok()?.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes()); // platformID: Windows
    out.extend_from_slice(&0u16.to_be_bytes()); // encodingID: symbol
    out.extend_from_slice(&u32::try_from(subtables_at).ok()?.to_be_bytes());
    let mut at = subtables_at.checked_add(symbol.len())?;
    for (platform, encoding, bytes) in &kept {
        out.extend_from_slice(&platform.to_be_bytes());
        out.extend_from_slice(&encoding.to_be_bytes());
        out.extend_from_slice(&u32::try_from(at).ok()?.to_be_bytes());
        at = at.checked_add(bytes.len())?;
    }
    out.extend_from_slice(&symbol);
    for (_, _, bytes) in &kept {
        out.extend_from_slice(bytes);
    }
    Some(out)
}

/// A format-4 subtable over `segments` (startCode, endCode, idDelta), every
/// idRangeOffset 0 and no glyphIdArray. `None` when the segment count or the
/// subtable size overflows the format's u16 fields.
fn format4_subtable(segments: &[(u16, u16, u16)]) -> Option<Vec<u8>> {
    let seg_count_x2 = segments.len().checked_mul(2)?;
    let mut search_range = 2usize; // 2 × 2^floor(log2(segCount))
    let mut entry_selector = 0u16;
    while search_range.checked_mul(2)? <= seg_count_x2 {
        search_range *= 2;
        entry_selector = entry_selector.checked_add(1)?;
    }
    let mut out = Vec::with_capacity(16 + 8 * segments.len());
    out.extend_from_slice(&4u16.to_be_bytes()); // format
    out.extend_from_slice(&0u16.to_be_bytes()); // length, patched below
    out.extend_from_slice(&0u16.to_be_bytes()); // language
    out.extend_from_slice(&u16::try_from(seg_count_x2).ok()?.to_be_bytes());
    out.extend_from_slice(&u16::try_from(search_range).ok()?.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&u16::try_from(seg_count_x2 - search_range).ok()?.to_be_bytes());
    for &(_, end, _) in segments {
        out.extend_from_slice(&end.to_be_bytes());
    }
    out.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    for &(start, _, _) in segments {
        out.extend_from_slice(&start.to_be_bytes());
    }
    for &(_, _, delta) in segments {
        out.extend_from_slice(&delta.to_be_bytes());
    }
    out.extend(std::iter::repeat_n(0, seg_count_x2)); // idRangeOffset: all zero
    let length = u16::try_from(out.len()).ok()?;
    out.get_mut(2..4)?.copy_from_slice(&length.to_be_bytes());
    Some(out)
}

/// The end of the cmap subtable starting at `start`, read from the subtable's
/// own `length` field (u16 at +2 for formats 0/2/4/6, u32 at +4 for 8/10/12/13
/// and at +2 for 14) and validated against the table's length.
fn subtable_end(data: &[u8], start: usize) -> Option<usize> {
    let format = usize::from(be16(data, start)?);
    let length = match format {
        8 | 10 | 12 | 13 => usize::try_from(be32(data, start.checked_add(4)?)?).ok()?,
        14 => usize::try_from(be32(data, start.checked_add(2)?)?).ok()?,
        _ => usize::from(be16(data, start.checked_add(2)?)?),
    };
    let end = start.checked_add(length)?;
    (end <= data.len()).then_some(end)
}

/// A big-endian u16 at `at`, `None` when the data is too short.
fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

/// A big-endian u32 at `at`, `None` when the data is too short.
fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The TrueType checksum of `data`: the big-endian u32 sum over zero-padded
/// 4-byte words (the same helper shape as `crates/fonts/src/pdf.rs`).
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, word| {
        let mut bytes = [0; 4];
        for (into, from) in bytes.iter_mut().zip(word) {
            *into = *from;
        }
        sum.wrapping_add(u32::from_be_bytes(bytes))
    })
}

/// Emit a complete sfnt from `tables`: the 12-byte header, one 16-byte record
/// per table in ascending tag order, each table's data zero-padded to 4 bytes,
/// per-table checksums (head's computed with `checkSumAdjustment` zeroed, as
/// the spec defines), and the whole-font `checkSumAdjustment` patched into
/// head. `None` when a count, offset or length overflows the directory's u16
/// and u32 fields, or when no head table exists to hold the adjustment. Task
/// 7's glyf/loca surgery reuses this writer for its own rewrites.
fn write_directory(sfnt_version: u32, tables: &mut [([u8; 4], Vec<u8>)]) -> Option<Vec<u8>> {
    if tables.is_empty() {
        return None;
    }
    tables.sort_by_key(|table| table.0);
    // The spec's head checksum runs over the table with checkSumAdjustment
    // zeroed; zero it now and patch the real value in at the end.
    for (tag, data) in tables.iter_mut() {
        if *tag == *b"head" {
            data.get_mut(8..12)?.fill(0);
        }
    }
    let directory_len = tables.len().checked_mul(16)?;
    let mut search_range = 16usize; // 16 × 2^floor(log2(numTables))
    let mut entry_selector = 0u16;
    while search_range.checked_mul(2)? <= directory_len {
        search_range *= 2;
        entry_selector = entry_selector.checked_add(1)?;
    }
    let mut offset = directory_len.checked_add(12)?;
    let mut head_offset = None;
    let mut records: Vec<([u8; 4], u32, u32, u32)> = Vec::with_capacity(tables.len()); // tag, checksum, offset, length
    for (tag, data) in tables.iter() {
        if *tag == *b"head" {
            head_offset = Some(offset);
        }
        records.push((*tag, checksum(data), u32::try_from(offset).ok()?, u32::try_from(data.len()).ok()?));
        offset = offset.checked_add(data.len().next_multiple_of(4))?;
    }
    let head_offset = head_offset?;
    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(&sfnt_version.to_be_bytes());
    out.extend_from_slice(&u16::try_from(tables.len()).ok()?.to_be_bytes());
    out.extend_from_slice(&u16::try_from(search_range).ok()?.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&u16::try_from(directory_len - search_range).ok()?.to_be_bytes());
    for (tag, sum, at, len) in records {
        out.extend_from_slice(&tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&at.to_be_bytes());
        out.extend_from_slice(&len.to_be_bytes());
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        let pad = data.len().next_multiple_of(4) - data.len();
        out.extend(std::iter::repeat_n(0, pad));
    }
    // checkSumAdjustment = 0xB1B0AFBA − wholeChecksum, both over the field
    // zeroed, so the filled font checksums to exactly the magic.
    let adjustment = CHECKSUM_MAGIC.wrapping_sub(checksum(&out));
    let at = head_offset.checked_add(8)?;
    out.get_mut(at..at.checked_add(4)?)?.copy_from_slice(&adjustment.to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::synthesize_cmap;

    fn be16(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }

    fn be32(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }

    /// A big-endian u16 at `at`, `None` when the slice is too short.
    fn be16_at(data: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
    }

    /// A big-endian u32 at `at`, `None` when the slice is too short.
    fn be32_at(data: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
    }

    /// The TrueType checksum of `data`: the big-endian u32 sum over zero-padded
    /// 4-byte words, as the spec defines it.
    fn checksum(data: &[u8]) -> u32 {
        data.chunks(4).fold(0u32, |sum, word| {
            let mut bytes = [0; 4];
            for (into, from) in bytes.iter_mut().zip(word) {
                *into = *from;
            }
            sum.wrapping_add(u32::from_be_bytes(bytes))
        })
    }

    /// The tables of the synthetic TrueType fixture from
    /// `crates/ps-export/src/fonts.rs::ttf_program`, duplicated per test module
    /// (the house style; `tests/oracle.rs` does the same): 1000 upem, three
    /// glyphs (empty outlines — these tests exercise bytes, not outlines), a
    /// (1,0) format-6 cmap mapping A→1 and B→2, and a version-3.0 `post`.
    /// Split into table list + assembly so a test can drop or corrupt a table.
    fn ttf_tables() -> Vec<(&'static [u8; 4], Vec<u8>)> {
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
        vec![(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"post", post)]
    }

    /// Directory emission like the fixture's own tail: 12-byte header, 16-byte
    /// records, each table zero-padded to 4 (checksums left zero — none of our
    /// readers check them on the input).
    fn assemble_ttf(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut search_range = 16usize;
        let mut entry_selector = 0u16;
        while search_range * 2 <= 16 * tables.len() {
            search_range *= 2;
            entry_selector += 1;
        }
        let mut out = Vec::new();
        out.extend_from_slice(&be32(0x0001_0000));
        out.extend_from_slice(&be16(tables.len() as u16));
        out.extend_from_slice(&be16(search_range as u16));
        out.extend_from_slice(&be16(entry_selector));
        out.extend_from_slice(&be16(u16::try_from(16 * tables.len() - search_range).expect("the range shift fits")));
        let mut offset = 12 + 16 * tables.len();
        let mut dir = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in tables {
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

    /// The assembled fixture, as `fonts.rs::ttf_program` builds it.
    fn ttf_program() -> Vec<u8> {
        assemble_ttf(&ttf_tables())
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

    /// The offset, within a `cmap` table, of the record for a
    /// (platform, encoding) pair.
    fn cmap_subtable(cmap: &[u8], platform: u16, encoding: u16) -> Option<usize> {
        let num = usize::from(be16_at(cmap, 2)?);
        for i in 0..num {
            let at = 4 + i * 8;
            if be16_at(cmap, at)? == platform && be16_at(cmap, at + 2)? == encoding {
                return usize::try_from(be32_at(cmap, at + 4)?).ok();
            }
        }
        None
    }

    /// Look `code` up in the format-4 subtable at `sub` (relative to the cmap
    /// table's start) the way a consumer does: find the segment, add `idDelta`.
    fn format4_gid(cmap: &[u8], sub: usize, code: u16) -> Option<u16> {
        let seg_count = usize::from(be16_at(cmap, sub + 6)?) / 2;
        let end_codes = sub + 14;
        let start_codes = end_codes + seg_count * 2 + 2;
        let id_deltas = start_codes + seg_count * 2;
        for i in 0..seg_count {
            let end = be16_at(cmap, end_codes + i * 2)?;
            let start = be16_at(cmap, start_codes + i * 2)?;
            if (start..=end).contains(&code) {
                let delta = be16_at(cmap, id_deltas + i * 2)?;
                return Some(code.wrapping_add(delta));
            }
        }
        None
    }

    /// Look `code` up in the format-6 subtable at `sub` (relative to the cmap
    /// table's start): firstCode + index into the glyph array.
    fn format6_gid(cmap: &[u8], sub: usize, code: u16) -> Option<u16> {
        let first = be16_at(cmap, sub + 6)?;
        let count = usize::from(be16_at(cmap, sub + 8)?);
        let index = usize::from(code.checked_sub(first)?);
        if index >= count {
            return None;
        }
        be16_at(cmap, sub + 10 + index * 2)
    }

    /// The synthesized (3,0) format-4 subtable maps the assigned codes to the
    /// requested glyph ids, an unmapped code stays unmapped, and the original
    /// subtable rides along so consumers can still reverse-map gid → Unicode.
    #[test]
    fn synthesized_cmap_maps_codes_to_gids() {
        let out = synthesize_cmap(&ttf_program(), &[(1, 1), (2, 2)]).expect("synthesis succeeds");
        let (off, len) = dir_entry(&out, b"cmap").expect("a cmap directory entry");
        let cmap = out.get(off..off + len).expect("the cmap table within the file");
        let symbol = cmap_subtable(cmap, 3, 0).expect("a (3,0) symbol record");
        assert_eq!(format4_gid(cmap, symbol, 1), Some(1), "code 1 → gid 1");
        assert_eq!(format4_gid(cmap, symbol, 2), Some(2), "code 2 → gid 2");
        assert_eq!(format4_gid(cmap, symbol, 3), None, "an unmapped code stays unmapped");
        let mac = cmap_subtable(cmap, 1, 0).expect("the original (1,0) record kept alongside");
        assert_eq!(format6_gid(cmap, mac, 65), Some(1), "'A' → gid 1 still resolves");
        assert_eq!(format6_gid(cmap, mac, 66), Some(2), "'B' → gid 2 still resolves");
    }

    /// The rebuilt directory parses as a font, keeps the table count, orders
    /// records by ascending tag, 4-aligns every offset and keeps every extent
    /// inside the file.
    #[test]
    fn directory_invariants_hold() {
        let out = synthesize_cmap(&ttf_program(), &[(1, 1), (2, 2)]).expect("synthesis succeeds");
        let font = skrifa::FontRef::from_index(&out, 0).expect("the output parses as a FontRef");
        let records = font.table_directory().table_records();
        assert_eq!(records.len(), 8, "cmap replaced one-for-one, every other table kept");
        let mut previous = None;
        for record in records {
            let tag = u32::from_be_bytes(record.tag().into_bytes());
            assert_eq!(record.offset() % 4, 0, "table offsets stay 4-aligned");
            let end = record.offset() as usize + record.length() as usize;
            assert!(end <= out.len(), "offset + length stays within the file");
            assert!(font.table_data(record.tag()).is_some(), "every table resolves through the provider");
            if let Some(previous) = previous {
                assert!(tag > previous, "records stay in ascending-tag order");
            }
            previous = Some(tag);
        }
    }

    /// Every per-table checksum is the big-endian u32 sum over the zero-padded
    /// table data (head's over the checkSumAdjustment zeroed, as the spec
    /// defines), and the whole-font checksum with the adjustment zeroed is
    /// 0xB1B0AFBA once the field is filled.
    #[test]
    fn checksums_are_correct() {
        let out = synthesize_cmap(&ttf_program(), &[(1, 1), (2, 2)]).expect("synthesis succeeds");
        let font = skrifa::FontRef::from_index(&out, 0).expect("the output parses as a FontRef");
        let mut head_at = None;
        for record in font.table_directory().table_records() {
            let offset = record.offset() as usize;
            let length = record.length() as usize;
            let mut data = out.get(offset..offset + length).expect("the table within the file").to_vec();
            if record.tag() == skrifa::Tag::new(b"head") {
                head_at = Some(offset);
                data[8..12].fill(0);
            }
            assert_eq!(checksum(&data), record.checksum(), "the checksum of {:?}", record.tag());
        }
        let head = head_at.expect("a head table holds the adjustment");
        // checkSumAdjustment is defined so the whole font, adjustment included,
        // checksums to exactly 0xB1B0AFBA once the field is filled.
        assert_eq!(checksum(&out), 0xB1B0_AFBA, "whole-font checksum with the adjustment filled");
        let mut zeroed = out.clone();
        zeroed[head + 8..head + 12].fill(0);
        let adjustment = u32::from_be_bytes(out[head + 8..head + 12].try_into().expect("the field"));
        assert_eq!(checksum(&zeroed).wrapping_add(adjustment), 0xB1B0_AFBA, "the filled value is the magic minus the zeroed checksum");
    }

    /// A font without a cmap gains exactly one: the missing table is inserted,
    /// the directory grows by one entry and the new subtable resolves codes.
    #[test]
    fn font_without_cmap_gains_one() {
        let tables = ttf_tables();
        let without: Vec<_> = tables.into_iter().filter(|(tag, _)| *tag != b"cmap").collect();
        let out = synthesize_cmap(&assemble_ttf(&without), &[(65, 1)]).expect("synthesis inserts a cmap");
        let font = skrifa::FontRef::from_index(&out, 0).expect("the output parses as a FontRef");
        assert_eq!(font.table_directory().num_tables(), 8, "seven original tables plus the new cmap");
        let (off, len) = dir_entry(&out, b"cmap").expect("a cmap directory entry");
        let cmap = out.get(off..off + len).expect("the cmap table within the file");
        let symbol = cmap_subtable(cmap, 3, 0).expect("a (3,0) symbol record");
        assert_eq!(format4_gid(cmap, symbol, 65), Some(1), "code 65 → gid 1");
    }

    /// Truncations at every interesting cut degrade to `None`, never a panic:
    /// empty, mid-header, mid-directory, exactly at the directory end, mid-body
    /// and one byte short of the whole file.
    #[test]
    fn truncated_font_returns_none() {
        let ttf = ttf_program();
        for cut in [0usize, 4, 12, 12 + 16 * 8, ttf.len() / 2, ttf.len() - 1] {
            let truncated = &ttf[..cut.min(ttf.len())];
            assert!(synthesize_cmap(truncated, &[(1, 1)]).is_none(), "a font truncated at {cut} must degrade, not crash");
        }
    }

    /// A directory record whose offset points past the end of the file makes
    /// the whole rewrite return `None`: the reader's bounds check answers for
    /// the table, and the caller degrades.
    #[test]
    fn offset_past_eof_returns_none() {
        let mut ttf = ttf_program();
        // Corrupt glyf's record (record 1 — the directory is alphabetical,
        // 16 bytes per record after the 12-byte header, offset at +8).
        let at = 12 + 16 + 8;
        let bogus = u32::try_from(ttf.len() + 0x100).expect("the length fits a u32");
        ttf[at..at + 4].copy_from_slice(&bogus.to_be_bytes());
        assert!(synthesize_cmap(&ttf, &[(1, 1)]).is_none(), "an out-of-range table offset must degrade, not crash");
    }
}
