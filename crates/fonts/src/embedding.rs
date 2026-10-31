//! Unicode embedding of an explicitly supplied, embeddable TrueType face. Font files are
//! document inputs or the optional craft-fonts build input, never system-interface fallbacks.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    raw::TableProvider,
};

/// Compare PDF subset/PostScript names with the full face's family name.
pub fn face_key(name: &str) -> String {
    name.rsplit('+').next().unwrap_or(name).chars().filter(char::is_ascii_alphanumeric).flat_map(char::to_lowercase).collect()
}

/// Full, approved faces of the source family. This never silently selects another family.
pub fn matching_faces(name: &str) -> Vec<&'static crate::CraftFont> {
    let key = face_key(name);
    crate::CRAFT_FONTS.iter().filter(|f| face_key(f.family) == key || face_key(&f.name()) == key).collect()
}

/// Whether a complete font program can show every non-newline character.
pub fn covers(bytes: &[u8], text: &str) -> bool {
    FontRef::new(bytes).is_ok_and(|f| text.chars().all(|c| matches!(c, '\n' | '\r') || f.charmap().map(c).is_some_and(|g| g.to_u32() != 0)))
}

/// Names come from the program, never a caller's guessed family.
pub fn names(bytes: &[u8]) -> Result<(String, String), String> {
    use skrifa::string::StringId;
    validate(bytes)?;
    let f = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let name = |id| {
        f.localized_strings(id)
            .find(|s| s.language() == Some("en"))
            .or_else(|| f.localized_strings(id).next())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty() && s.len() <= 128)
    };
    let family = name(StringId::FAMILY_NAME).ok_or("the font has no family name")?;
    let postscript = name(StringId::POSTSCRIPT_NAME).unwrap_or_else(|| family.replace(' ', ""));
    Ok((family, postscript))
}

/// A selected external font is document input. Require its own open-licence declaration;
/// never turn a proprietary system-interface fallback into an exported PDF font.
pub fn validate_document_input(bytes: &[u8]) -> Result<(), String> {
    use skrifa::string::StringId;
    let (family, _) = names(bytes)?;
    let key = face_key(&family);
    if ["sourcehan", "sourceserif", "sourcesans", "notosanscjk", "notoserifcjk"].iter().any(|n| key.starts_with(n)) {
        return Err("this font family is excluded by the project's asset policy".into());
    }
    let f = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let attribution: String =
        [StringId::COPYRIGHT_NOTICE, StringId::MANUFACTURER].into_iter().flat_map(|id| f.localized_strings(id)).map(|s| s.to_string()).collect();
    if attribution.to_ascii_lowercase().contains("adobe") {
        return Err("Adobe-authored font assets are excluded by the project's asset policy".into());
    }
    let license: String = f.localized_strings(StringId::LICENSE_DESCRIPTION).map(|s| s.to_string()).collect();
    if !["SIL OPEN FONT", "SIL Open Font", "Apache License", "Ubuntu Font Licence", "Bitstream Vera", "MIT License"]
        .iter()
        .any(|l| license.contains(l))
    {
        return Err("select an openly licensed font whose name table declares its licence (OFL, Apache, Ubuntu, Bitstream Vera or MIT)".into());
    }
    validate(bytes)?;
    Ok(())
}

/// Validate before allocating any PDF objects, so an invalid font cannot partly mutate a doc.
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 16 << 20 {
        return Err("the selected font exceeds 16 MiB".into());
    }
    let font = FontRef::new(bytes).map_err(|_| "select a standalone TrueType/OpenType font (not a font collection)")?;
    if font.fvar().is_ok() {
        return Err("select a static instance of this variable font before embedding".into());
    }
    if (font.glyf().is_err() && font.cff().is_err()) || font.head().is_err() || font.hhea().is_err() || font.hmtx().is_err() {
        return Err("the selected font has no usable TrueType/CFF outlines and metrics".into());
    }
    if font.os2().is_ok_and(|t| t.fs_type() & (0x0002 | 0x0004 | 0x0200) != 0) {
        return Err("this font's embedding permissions prohibit editable outline embedding".into());
    }
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    if m.units_per_em == 0 || font.charmap().mappings().next().is_none() {
        return Err("the selected font has no Unicode mapping or valid units per em".into());
    }
    if font.charmap().mappings().take(65_535).count() >= 65_535 {
        return Err("the selected font has too many Unicode mappings for one PDF font".into());
    }
    if font.charmap().mappings().any(|(_, g)| g.to_u32() >= u32::from(m.glyph_count)) {
        return Err("the selected font maps Unicode to a glyph outside its program".into());
    }
    Ok(())
}

fn stream(doc: &mut Document, bytes: Vec<u8>) -> Object {
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), bytes))))
}

/// Embed the complete font once and provide separate CIDs for Unicode aliases. /ToUnicode,
/// /CIDToGIDMap and widths agree, including supplementary characters, so reopening needs no
/// installed font. Keeping the full program also lets subsequent edits add other glyphs.
pub fn embed(doc: &mut Document, fonts: &mut Dict, bytes: &[u8], family: &str) -> Result<String, String> {
    validate(bytes)?;
    let font = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let mappings: Vec<_> = font
        .charmap()
        .mappings()
        .filter(|(_, gid)| gid.to_u32() > 0)
        .filter_map(|(cp, gid)| char::from_u32(cp).map(|c| (c.to_string(), gid)))
        .collect();
    embed_mapping(doc, fonts, bytes, family, &mappings)
}

/// Explicit glyph mapping for shaped text. A CID can represent a contextual glyph or a
/// ligature, while its ToUnicode retains the logical cluster. The full source program stays exact.
pub fn embed_mapping(
    doc: &mut Document,
    fonts: &mut Dict,
    bytes: &[u8],
    family: &str,
    mappings: &[(String, skrifa::GlyphId)],
) -> Result<String, String> {
    validate(bytes)?;
    if mappings.len() > 65_534 || mappings.iter().any(|(s, _)| s.len() > 4096) {
        return Err("shaped font mapping exceeds its budget".into());
    }
    let font = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let cff = font.glyf().is_err();
    let loc = LocationRef::default();
    let m = font.metrics(Size::unscaled(), loc);
    let scale = 1000.0 / f64::from(m.units_per_em);
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    let mut cid_encoding = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PCGlyphs def\n/CMapType 1 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let mut cff_widths = std::collections::BTreeMap::new();
    let mut gids = vec![0, 0];
    let mut widths = Vec::with_capacity(mappings.len());
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PCUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for (chunk_index, chunk) in mappings.chunks(100).enumerate() {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        let _ = writeln!(cid_encoding, "{} begincidchar", chunk.len());
        for (offset, (unicode, gid)) in chunk.iter().enumerate() {
            let cid = u16::try_from(chunk_index.saturating_mul(100).saturating_add(offset).saturating_add(1)).map_err(|_| "font mapping limit")?;
            let glyph = u16::try_from(gid.to_u32()).map_err(|_| "font glyph limit")?;
            if glyph == 0 || u32::from(glyph) >= u32::from(m.glyph_count) {
                return Err("font contains an unavailable shaped glyph".into());
            }
            let width = advances.advance_width(*gid).unwrap_or(0.0) as f64 * scale;
            if !width.is_finite() || !(0.0..=16_000.0).contains(&width) {
                return Err("font contains an invalid advance".into());
            }
            gids.extend_from_slice(&glyph.to_be_bytes());
            widths.push(Object::Real(width));
            cff_widths.insert(glyph, width);
            let _ = writeln!(cid_encoding, "<{cid:04X}> {glyph}");
            let hex: String = unicode.encode_utf16().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{cid:04X}> <{hex}>");
        }
        cmap.push_str("endbfchar\n");
        cid_encoding.push_str("endcidchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cid_encoding.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let name = family.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect::<String>();
    if name.is_empty() || name.len() > 128 {
        return Err("the font family name is invalid".into());
    }
    let glyph_map = if cff { cid_encoding.as_bytes() } else { gids.as_slice() };
    let mut programs = ProgramMatch::new(bytes);
    if let Some(resource) = same_resource(doc, fonts, &name, &mut programs, glyph_map, cmap.as_bytes()) {
        return Ok(resource);
    }
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&name));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Real(f64::from(m.italic_angle)));
    descriptor.set(b"Ascent".to_vec(), Object::Real(f64::from(m.ascent) * scale));
    descriptor.set(b"Descent".to_vec(), Object::Real(f64::from(m.descent) * scale));
    descriptor.set(b"CapHeight".to_vec(), Object::Real(f64::from(m.cap_height.unwrap_or(m.ascent)) * scale));
    descriptor.set(b"StemV".to_vec(), Object::Int(80));
    let bounds = m.bounds.ok_or("font has no outline bounds")?;
    descriptor.set(
        b"FontBBox".to_vec(),
        Object::Array([bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max].map(|v| Object::Real(f64::from(v) * scale)).to_vec()),
    );
    // Reuse the program another resource of this page already has, else one registered on another
    // page; source subset programs stay untouched.
    let existing = existing_program(doc, fonts, &mut programs);
    let program = existing.unwrap_or_else(|| {
        let mut d = Dict::new();
        d.set(b"Length1".to_vec(), Object::Int(bytes.len() as i64));
        if cff {
            d.set(b"Subtype".to_vec(), Object::name("OpenType"));
        }
        d.set(b"PCFullFont".to_vec(), Object::Bool(true));
        Object::Ref(doc.add(Object::Stream(Stream::flate(d, bytes))))
    });
    descriptor.set(if cff { b"FontFile3".to_vec() } else { b"FontFile2".to_vec() }, program);
    let desc = Object::Ref(doc.add(Object::Dict(descriptor)));
    let mut system = Dict::new();
    system.set(b"Registry".to_vec(), Object::String(PdfString::literal(b"Adobe".to_vec())));
    system.set(b"Ordering".to_vec(), Object::String(PdfString::literal(b"Identity".to_vec())));
    system.set(b"Supplement".to_vec(), Object::Int(0));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name(if cff { "CIDFontType0" } else { "CIDFontType2" }));
    cid.set(b"BaseFont".to_vec(), Object::name(&name));
    cid.set(b"CIDSystemInfo".to_vec(), Object::Dict(system));
    cid.set(b"FontDescriptor".to_vec(), desc);
    if cff {
        cid.set(
            b"W".to_vec(),
            Object::Array(cff_widths.into_iter().flat_map(|(gid, w)| [Object::Int(i64::from(gid)), Object::Array(vec![Object::Real(w)])]).collect()),
        );
    } else {
        cid.set(b"CIDToGIDMap".to_vec(), stream(doc, gids));
        cid.set(b"W".to_vec(), Object::Array(vec![Object::Int(1), Object::Array(widths)]));
    }
    let descendant = Object::Ref(doc.add(Object::Dict(cid)));
    let mut top = Dict::new();
    top.set(b"Type".to_vec(), Object::name("Font"));
    top.set(b"Subtype".to_vec(), Object::name("Type0"));
    top.set(b"BaseFont".to_vec(), Object::name(&name));
    top.set(b"Encoding".to_vec(), if cff { stream(doc, cid_encoding.into_bytes()) } else { Object::name("Identity-H") });
    top.set(b"DescendantFonts".to_vec(), Object::Array(vec![descendant]));
    top.set(b"ToUnicode".to_vec(), stream(doc, cmap.into_bytes()));
    let mut index = 0usize;
    while fonts.contains(format!("PCFull{index}").as_bytes()) {
        index = index.saturating_add(1);
    }
    let resource = format!("PCFull{index}");
    fonts.set(resource.as_bytes().to_vec(), Object::Ref(doc.add(Object::Dict(top))));
    Ok(resource)
}

/// Whether the stream `object` refers to decodes to `expected`.
fn stream_is(doc: &Document, object: Option<&Object>, expected: &[u8]) -> bool {
    object.is_some_and(|o| match doc.resolve(o).as_ref() {
        Object::Stream(s) => s.decoded_within(expected.len()).is_ok_and(|d| d == expected),
        _ => false,
    })
}

/// Full programs one embedding decodes at most while looking for its own. A real match passes
/// the cheap checks and decodes once; many candidates that pass them come from a hostile file.
const MAX_PROGRAM_DECODES: usize = 16;

/// Recognises the full program written for `bytes`: each candidate stream decodes at most once
/// and within `bytes.len()`, and at most [`MAX_PROGRAM_DECODES`] decode in all.
struct ProgramMatch<'a> {
    bytes: &'a [u8],
    known: HashMap<ObjRef, bool>,
    decodes: usize,
}

impl<'a> ProgramMatch<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, known: HashMap::new(), decodes: 0 }
    }

    /// Whether `r`, holding `object`, is the program.
    fn is(&mut self, doc: &Document, r: ObjRef, object: &Object) -> bool {
        if let Some(&known) = self.known.get(&r) {
            return known;
        }
        let found = match object {
            Object::Stream(s)
                if s.dict.get(b"PCFullFont").is_some()
                    && s.dict.get(b"Length1").and_then(|l| doc.resolve(l).as_int()) == i64::try_from(self.bytes.len()).ok()
                    && self.decodes < MAX_PROGRAM_DECODES =>
            {
                self.decodes = self.decodes.saturating_add(1);
                s.decoded_within(self.bytes.len()).is_ok_and(|data| data == self.bytes)
            }
            _ => false,
        };
        self.known.insert(r, found);
        found
    }

    /// Whether the resource's program is this one.
    fn written(&mut self, doc: &Document, w: &Written) -> bool {
        w.program.as_ref().is_some_and(|r| self.is(doc, r, &doc.get(r)))
    }
}

/// The program for `programs.bytes` that this page's `PCFull` resources, or else the first
/// objects of the document, already have.
fn existing_program(doc: &Document, fonts: &Dict, programs: &mut ProgramMatch) -> Option<Object> {
    written(doc, fonts)
        .find(|w| programs.written(doc, w))
        .map(|w| w.program)
        .or_else(|| doc.scan_objects().take(8192).find_map(|(r, o)| programs.is(doc, r, &o).then_some(Object::Ref(r))))
}

/// A `PCFull` resource that [`embed_mapping`] wrote.
struct Written {
    resource: String,
    top: Dict,
    cid: Dict,
    /// The reference to its full program.
    program: Object,
}

/// The `PCFull` resources in `fonts`, in order.
fn written<'a>(doc: &'a Document, fonts: &'a Dict) -> impl Iterator<Item = Written> + 'a {
    fonts.iter().filter(|(key, _)| key.starts_with(b"PCFull")).take(256).filter_map(move |(key, value)| {
        let top = doc.dict(value)?;
        let cid = doc.dict(doc.resolve(top.get(b"DescendantFonts")?).as_array()?.first()?)?;
        let descriptor = doc.dict(cid.get(b"FontDescriptor")?)?;
        let program = descriptor.get(b"FontFile2").or_else(|| descriptor.get(b"FontFile3")).filter(|p| matches!(p, Object::Ref(_)))?.clone();
        Some(Written { resource: String::from_utf8_lossy(key).into_owned(), top, cid, program })
    })
}

/// A resource in `fonts` that already is this embedding: the same font name, program, glyph map
/// and Unicode map. Editing the same text again reuses it instead of adding another copy.
fn same_resource(doc: &Document, fonts: &Dict, name: &str, programs: &mut ProgramMatch, glyph_map: &[u8], unicode: &[u8]) -> Option<String> {
    written(doc, fonts)
        .find(|w| {
            let glyphs = w.cid.get(b"CIDToGIDMap").or_else(|| w.top.get(b"Encoding"));
            w.top.name(b"BaseFont") == Some(name.as_bytes())
                && stream_is(doc, w.top.get(b"ToUnicode"), unicode)
                && stream_is(doc, glyphs, glyph_map)
                && programs.written(doc, w)
        })
        .map(|w| w.resource)
}

/// Generate a 6-character subset prefix tag for a font name, e.g. "ABCDEF+".
pub fn subset_tag(seed: u64) -> String {
    let mut tag = String::with_capacity(7);
    let mut s = seed;
    for _ in 0..6 {
        let b = b'A'.saturating_add((s % 26) as u8);
        tag.push(b as char);
        s /= 26;
    }
    tag.push('+');
    tag
}

fn calc_table_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        if let Some(target) = b.get_mut(..chunk.len()) {
            target.copy_from_slice(chunk);
        }
        sum = sum.wrapping_add(u32::from_be_bytes(b));
    }
    sum
}

fn read_tables(bytes: &[u8]) -> Option<HashMap<[u8; 4], &[u8]>> {
    let count_bytes: [u8; 2] = bytes.get(4..6)?.try_into().ok()?;
    let num_tables = usize::from(u16::from_be_bytes(count_bytes));
    if num_tables > 128 {
        return None;
    }
    let mut map = HashMap::new();
    for i in 0..num_tables {
        let entry = 12usize.checked_add(i.checked_mul(16)?)?;
        let tag: [u8; 4] = bytes.get(entry..entry.checked_add(4)?)?.try_into().ok()?;
        let off_bytes: [u8; 4] = bytes.get(entry.checked_add(8)?..entry.checked_add(12)?)?.try_into().ok()?;
        let offset = usize::try_from(u32::from_be_bytes(off_bytes)).ok()?;
        let len_bytes: [u8; 4] = bytes.get(entry.checked_add(12)?..entry.checked_add(16)?)?.try_into().ok()?;
        let length = usize::try_from(u32::from_be_bytes(len_bytes)).ok()?;
        let table_data = bytes.get(offset..offset.checked_add(length)?)?;
        map.insert(tag, table_data);
    }
    Some(map)
}

fn glyph_slice<'a>(loca: &'a [u8], glyf: &'a [u8], is_long: bool, gid: u16) -> Option<&'a [u8]> {
    let (start, end) = if is_long {
        let s_idx = usize::from(gid).checked_mul(4)?;
        let e_idx = s_idx.checked_add(4)?;
        let start_bytes: [u8; 4] = loca.get(s_idx..e_idx)?.try_into().ok()?;
        let end_bytes: [u8; 4] = loca.get(e_idx..e_idx.checked_add(4)?)?.try_into().ok()?;
        (usize::try_from(u32::from_be_bytes(start_bytes)).ok()?, usize::try_from(u32::from_be_bytes(end_bytes)).ok()?)
    } else {
        let s_idx = usize::from(gid).checked_mul(2)?;
        let e_idx = s_idx.checked_add(2)?;
        let start_bytes: [u8; 2] = loca.get(s_idx..e_idx)?.try_into().ok()?;
        let end_bytes: [u8; 2] = loca.get(e_idx..e_idx.checked_add(2)?)?.try_into().ok()?;
        (usize::from(u16::from_be_bytes(start_bytes)).checked_mul(2)?, usize::from(u16::from_be_bytes(end_bytes)).checked_mul(2)?)
    };
    if start <= end && end <= glyf.len() { glyf.get(start..end) } else { None }
}

fn composite_components(data: &[u8]) -> Vec<u16> {
    let mut comps = Vec::new();
    if data.len() < 10 {
        return comps;
    }
    let Some(header) = data.get(0..2) else { return comps };
    let num_contours = i16::from_be_bytes([header[0], header[1]]);
    if num_contours != -1 {
        return comps;
    }
    let mut offset = 10usize;
    while let Some(flags_bytes) = data.get(offset..offset.saturating_add(2)) {
        if flags_bytes.len() < 2 {
            break;
        }
        let flags = u16::from_be_bytes([flags_bytes[0], flags_bytes[1]]);
        let Some(gid_bytes) = data.get(offset.saturating_add(2)..offset.saturating_add(4)) else { break };
        if gid_bytes.len() < 2 {
            break;
        }
        let gid = u16::from_be_bytes([gid_bytes[0], gid_bytes[1]]);
        comps.push(gid);
        let Some(mut next_offset) = offset.checked_add(4) else { break };
        if flags & 0x0001 != 0 {
            next_offset = match next_offset.checked_add(4) {
                Some(o) => o,
                None => break,
            };
        } else {
            next_offset = match next_offset.checked_add(2) {
                Some(o) => o,
                None => break,
            };
        }
        if flags & 0x0008 != 0 {
            next_offset = match next_offset.checked_add(2) {
                Some(o) => o,
                None => break,
            };
        } else if flags & 0x0040 != 0 {
            next_offset = match next_offset.checked_add(4) {
                Some(o) => o,
                None => break,
            };
        } else if flags & 0x0080 != 0 {
            next_offset = match next_offset.checked_add(8) {
                Some(o) => o,
                None => break,
            };
        }
        offset = next_offset;
        if flags & 0x0020 == 0 {
            break;
        }
    }
    comps
}

fn remap_composite_glyph(data: &[u8], old_to_new: &HashMap<u16, u16>) -> Vec<u8> {
    let mut out = data.to_vec();
    if out.len() < 10 {
        return out;
    }
    let Some(header) = out.get(0..2) else { return out };
    let num_contours = i16::from_be_bytes([header[0], header[1]]);
    if num_contours != -1 {
        return out;
    }
    let mut offset = 10usize;
    while let Some(flags_bytes) = out.get(offset..offset.saturating_add(2)) {
        if flags_bytes.len() < 2 {
            break;
        }
        let flags = u16::from_be_bytes([flags_bytes[0], flags_bytes[1]]);
        let Some(gid_bytes) = out.get(offset.saturating_add(2)..offset.saturating_add(4)) else { break };
        if gid_bytes.len() < 2 {
            break;
        }
        let old_gid = u16::from_be_bytes([gid_bytes[0], gid_bytes[1]]);
        let new_gid = old_to_new.get(&old_gid).copied().unwrap_or(0);
        if let Some(slot) = out.get_mut(offset.saturating_add(2)..offset.saturating_add(4)) {
            slot.copy_from_slice(&new_gid.to_be_bytes());
        }
        let Some(mut next_offset) = offset.checked_add(4) else { break };
        if flags & 0x0001 != 0 {
            next_offset = match next_offset.checked_add(4) {
                Some(o) => o,
                None => break,
            };
        } else {
            next_offset = match next_offset.checked_add(2) {
                Some(o) => o,
                None => break,
            };
        }
        if flags & 0x0008 != 0 {
            next_offset = match next_offset.checked_add(2) {
                Some(o) => o,
                None => break,
            };
        } else if flags & 0x0040 != 0 {
            next_offset = match next_offset.checked_add(4) {
                Some(o) => o,
                None => break,
            };
        } else if flags & 0x0080 != 0 {
            next_offset = match next_offset.checked_add(8) {
                Some(o) => o,
                None => break,
            };
        }
        offset = next_offset;
        if flags & 0x0020 == 0 {
            break;
        }
    }
    out
}

fn build_name_table(ps_name: &str) -> Vec<u8> {
    let utf16_be: Vec<u8> = ps_name.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let str_len = u16::try_from(utf16_be.len()).unwrap_or(0);
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&18u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0x0409u16.to_be_bytes());
    out.extend_from_slice(&6u16.to_be_bytes());
    out.extend_from_slice(&str_len.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&utf16_be);
    out
}

fn build_cmap_table(char_to_new_gid: &[(u32, u32)]) -> Vec<u8> {
    let mut sorted = char_to_new_gid.to_vec();
    if sorted.is_empty() {
        sorted.push((0, 0));
    }
    sorted.sort_by_key(|&(c, _)| c);
    sorted.dedup_by_key(|&mut (c, _)| c);
    let n_groups = u32::try_from(sorted.len()).unwrap_or(1);
    let subtable_len = 16u32.saturating_add(n_groups.saturating_mul(12));
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&10u16.to_be_bytes());
    out.extend_from_slice(&12u32.to_be_bytes());
    out.extend_from_slice(&12u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&subtable_len.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&n_groups.to_be_bytes());
    for &(c, gid) in &sorted {
        out.extend_from_slice(&c.to_be_bytes());
        out.extend_from_slice(&c.to_be_bytes());
        out.extend_from_slice(&gid.to_be_bytes());
    }
    out
}

/// Subset a TrueType font to only the specified glyph IDs.
/// Returns the new sfnt font binary and a mapping from old GID to new GID.
#[allow(clippy::cognitive_complexity)]
pub fn subset_truetype(bytes: &[u8], used_gids: &[u16], postscript_name: &str) -> Result<(Vec<u8>, HashMap<u16, u16>), String> {
    validate(bytes)?;
    let map = read_tables(bytes).ok_or("cannot parse font table directory")?;
    let orig_glyf = map.get(b"glyf").copied().ok_or("the font has no TrueType glyf table")?;
    let orig_loca = map.get(b"loca").copied().ok_or("missing loca table")?;
    let orig_head = map.get(b"head").copied().ok_or("missing head table")?;
    let orig_hhea = map.get(b"hhea").copied().ok_or("missing hhea table")?;
    let orig_hmtx = map.get(b"hmtx").copied().ok_or("missing hmtx table")?;
    let orig_maxp = map.get(b"maxp").copied().ok_or("missing maxp table")?;

    let index_fmt_bytes: [u8; 2] = orig_head.get(50..52).ok_or("invalid head table")?.try_into().map_err(|_| "invalid head table")?;
    let is_long = i16::from_be_bytes(index_fmt_bytes) != 0;

    let mut needed_gids = BTreeSet::new();
    needed_gids.insert(0u16);
    for &g in used_gids {
        needed_gids.insert(g);
    }
    let mut queue: Vec<u16> = needed_gids.iter().copied().collect();
    while let Some(gid) = queue.pop() {
        if let Some(g_data) = glyph_slice(orig_loca, orig_glyf, is_long, gid) {
            for comp in composite_components(g_data) {
                if needed_gids.insert(comp) {
                    queue.push(comp);
                }
            }
        }
    }

    let mut subset_glyphs = Vec::with_capacity(needed_gids.len());
    let mut old_to_new_gid = HashMap::new();
    for (new_gid, &old_gid) in needed_gids.iter().enumerate() {
        let new_gid_u16 = u16::try_from(new_gid).map_err(|_| "too many glyphs in subset")?;
        old_to_new_gid.insert(old_gid, new_gid_u16);
        subset_glyphs.push(old_gid);
    }

    let mut subset_glyf = Vec::new();
    let mut subset_loca = Vec::with_capacity((subset_glyphs.len().saturating_add(1)).saturating_mul(4));
    for &old_gid in &subset_glyphs {
        let offset_u32 = u32::try_from(subset_glyf.len()).map_err(|_| "glyf table overflow")?;
        subset_loca.extend_from_slice(&offset_u32.to_be_bytes());
        if let Some(g_data) = glyph_slice(orig_loca, orig_glyf, is_long, old_gid)
            && !g_data.is_empty()
        {
            let remapped = remap_composite_glyph(g_data, &old_to_new_gid);
            subset_glyf.extend_from_slice(&remapped);
            if subset_glyf.len() % 2 != 0 {
                subset_glyf.push(0);
            }
        }
    }
    let final_offset_u32 = u32::try_from(subset_glyf.len()).map_err(|_| "glyf table overflow")?;
    subset_loca.extend_from_slice(&final_offset_u32.to_be_bytes());

    let hmetrics_bytes: [u8; 2] = orig_hhea.get(34..36).ok_or("invalid hhea")?.try_into().map_err(|_| "invalid hhea")?;
    let orig_num_hmetrics = usize::from(u16::from_be_bytes(hmetrics_bytes));
    let mut subset_hmtx = Vec::with_capacity(subset_glyphs.len().saturating_mul(4));
    for &old_gid in &subset_glyphs {
        let old_idx = usize::from(old_gid);
        let (adv, lsb) = if old_idx < orig_num_hmetrics {
            let adv_bytes: [u8; 2] = orig_hmtx.get(old_idx * 4..old_idx * 4 + 2).unwrap_or(&[0, 0]).try_into().unwrap_or([0, 0]);
            let lsb_bytes: [u8; 2] = orig_hmtx.get(old_idx * 4 + 2..old_idx * 4 + 4).unwrap_or(&[0, 0]).try_into().unwrap_or([0, 0]);
            (u16::from_be_bytes(adv_bytes), i16::from_be_bytes(lsb_bytes))
        } else {
            let last_adv_offset = orig_num_hmetrics.saturating_sub(1) * 4;
            let adv_bytes: [u8; 2] = orig_hmtx.get(last_adv_offset..last_adv_offset + 2).unwrap_or(&[0, 0]).try_into().unwrap_or([0, 0]);
            let lsb_offset = orig_num_hmetrics * 4 + (old_idx.saturating_sub(orig_num_hmetrics)) * 2;
            let lsb_bytes: [u8; 2] = orig_hmtx.get(lsb_offset..lsb_offset + 2).unwrap_or(&[0, 0]).try_into().unwrap_or([0, 0]);
            (u16::from_be_bytes(adv_bytes), i16::from_be_bytes(lsb_bytes))
        };
        subset_hmtx.extend_from_slice(&adv.to_be_bytes());
        subset_hmtx.extend_from_slice(&lsb.to_be_bytes());
    }

    let mut subset_hhea = orig_hhea.to_vec();
    if subset_hhea.len() >= 36 {
        let num_metrics = u16::try_from(subset_glyphs.len()).map_err(|_| "too many glyphs")?;
        if let Some(target) = subset_hhea.get_mut(34..36) {
            target.copy_from_slice(&num_metrics.to_be_bytes());
        }
    }

    let mut subset_maxp = orig_maxp.to_vec();
    if subset_maxp.len() >= 6 {
        let num_g = u16::try_from(subset_glyphs.len()).map_err(|_| "too many glyphs")?;
        if let Some(target) = subset_maxp.get_mut(4..6) {
            target.copy_from_slice(&num_g.to_be_bytes());
        }
    }

    let mut subset_head = orig_head.to_vec();
    if subset_head.len() >= 54 {
        if let Some(target) = subset_head.get_mut(8..12) {
            target.copy_from_slice(&[0, 0, 0, 0]);
        }
        if let Some(target) = subset_head.get_mut(50..52) {
            target.copy_from_slice(&1i16.to_be_bytes());
        }
    }

    let mut subset_post = vec![0u8; 32];
    if let Some(target) = subset_post.get_mut(0..4) {
        target.copy_from_slice(&0x00030000u32.to_be_bytes());
    }
    if let Some(orig_post) = map.get(b"post")
        && orig_post.len() >= 16
        && let (Some(target), Some(src)) = (subset_post.get_mut(4..16), orig_post.get(4..16))
    {
        target.copy_from_slice(src);
    }

    let char_to_new: Vec<(u32, u32)> = FontRef::new(bytes)
        .ok()
        .map(|face| {
            face.charmap().mappings().filter_map(|(cp, old)| old_to_new_gid.get(&(old.to_u32() as u16)).map(|&new| (cp, u32::from(new)))).collect()
        })
        .unwrap_or_default();

    let subset_name = build_name_table(postscript_name);
    let subset_cmap = build_cmap_table(&char_to_new);

    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"cmap", subset_cmap),
        (*b"glyf", subset_glyf),
        (*b"head", subset_head),
        (*b"hhea", subset_hhea),
        (*b"hmtx", subset_hmtx),
        (*b"loca", subset_loca),
        (*b"maxp", subset_maxp),
        (*b"name", subset_name),
    ];
    if let Some(os2) = map.get(b"OS/2") {
        tables.push((*b"OS/2", os2.to_vec()));
    }
    if let Some(cvt) = map.get(b"cvt ") {
        tables.push((*b"cvt ", cvt.to_vec()));
    }
    if let Some(fpgm) = map.get(b"fpgm") {
        tables.push((*b"fpgm", fpgm.to_vec()));
    }
    if let Some(prep) = map.get(b"prep") {
        tables.push((*b"prep", prep.to_vec()));
    }
    tables.push((*b"post", subset_post));

    tables.sort_by_key(|t| t.0);

    let num_tables = u16::try_from(tables.len()).map_err(|_| "too many tables")?;
    let mut max_pow2 = 1u16;
    let mut entry_selector = 0u16;
    while max_pow2.saturating_mul(2) <= num_tables {
        max_pow2 = max_pow2.saturating_mul(2);
        entry_selector = entry_selector.saturating_add(1);
    }
    let search_range = max_pow2.saturating_mul(16);
    let range_shift = num_tables.saturating_mul(16).saturating_sub(search_range);

    let mut sfnt = Vec::new();
    sfnt.extend_from_slice(&0x00010000u32.to_be_bytes());
    sfnt.extend_from_slice(&num_tables.to_be_bytes());
    sfnt.extend_from_slice(&search_range.to_be_bytes());
    sfnt.extend_from_slice(&entry_selector.to_be_bytes());
    sfnt.extend_from_slice(&range_shift.to_be_bytes());

    let dir_size = 12usize.saturating_add(usize::from(num_tables).saturating_mul(16));
    let mut current_offset = dir_size;

    struct Rec {
        tag: [u8; 4],
        checksum: u32,
        offset: u32,
        length: u32,
    }
    let mut records = Vec::with_capacity(tables.len());
    for (tag, data) in &tables {
        let checksum = calc_table_checksum(data);
        let offset = u32::try_from(current_offset).map_err(|_| "font offset overflow")?;
        let length = u32::try_from(data.len()).map_err(|_| "font table overflow")?;
        records.push(Rec { tag: *tag, checksum, offset, length });
        let padded_len = (data.len().saturating_add(3)) & !3;
        current_offset = current_offset.saturating_add(padded_len);
    }

    for r in &records {
        sfnt.extend_from_slice(&r.tag);
        sfnt.extend_from_slice(&r.checksum.to_be_bytes());
        sfnt.extend_from_slice(&r.offset.to_be_bytes());
        sfnt.extend_from_slice(&r.length.to_be_bytes());
    }

    let mut head_offset = None;
    for (r, (_, data)) in records.iter().zip(&tables) {
        if &r.tag == b"head" {
            head_offset = Some(sfnt.len());
        }
        sfnt.extend_from_slice(data);
        while sfnt.len() % 4 != 0 {
            sfnt.push(0);
        }
    }

    if let Some(h_off) = head_offset {
        let file_checksum = calc_table_checksum(&sfnt);
        let adjustment = 0xB1B0AFBAu32.wrapping_sub(file_checksum);
        if let Some(slot) = sfnt.get_mut(h_off.saturating_add(8)..h_off.saturating_add(12)) {
            slot.copy_from_slice(&adjustment.to_be_bytes());
        }
    }

    Ok((sfnt, old_to_new_gid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::Metrics;

    #[test]
    fn full_unicode_embedding_keeps_outlines_advances_and_roundtrip_codes() {
        let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        let name = embed(&mut doc, &mut fonts, bytes, "Inter").unwrap();
        let font = doc.dict(fonts.get(name.as_bytes()).unwrap()).unwrap();
        let m = Metrics::from_dict(&doc, &font);
        let text = "Aé ΩЖ €";
        let codes = m.encode(text).unwrap();
        assert_eq!(m.decode(&codes), text);
        assert_eq!(crate::pdf::embedded_sfnt(&doc, &font).unwrap(), bytes);
        let sfnt = FontRef::new(bytes).unwrap();
        for (ch, (code, _)) in text.chars().zip(m.codes(&codes)) {
            let gid = sfnt.charmap().map(ch).unwrap();
            let w = sfnt.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(gid).unwrap() as f64
                / f64::from(sfnt.metrics(Size::unscaled(), LocationRef::default()).units_per_em);
            assert!((m.width(code) - w).abs() < 1e-8);
        }
        let before = doc.object_numbers().len();
        assert!(embed(&mut doc, &mut fonts, b"broken font", "Broken").is_err());
        assert!(embed_mapping(&mut doc, &mut fonts, bytes, "Inter", &[("X".into(), skrifa::GlyphId::new(65_535))]).is_err());
        assert_eq!(doc.object_numbers().len(), before);
        let programs = || doc.scan_objects().filter(|(_, o)| matches!(o.as_ref(), Object::Stream(s) if s.dict.get(b"PCFullFont").is_some())).count();
        assert_eq!(programs(), 1);
        let mut second = Dict::new();
        embed(&mut doc, &mut second, bytes, "Inter").unwrap();
        assert_eq!(doc.scan_objects().filter(|(_, o)| matches!(o.as_ref(), Object::Stream(s) if s.dict.get(b"PCFullFont").is_some())).count(), 1);
        assert_eq!(face_key("ABCDEF+Droid-Sans-Fallback"), face_key("Droid Sans Fallback"));
    }

    #[test]
    fn embedding_the_same_text_again_reuses_its_resource_and_program() {
        let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let face = FontRef::new(bytes).unwrap();
        let glyph = |c| face.charmap().map(c).unwrap();
        let programs = |doc: &Document| {
            doc.scan_objects().filter(|(_, o)| matches!(o.as_ref(), Object::Stream(s) if s.dict.get(b"PCFullFont").is_some())).count()
        };
        // Objects before the program push it past the first 8192 that the document-wide search reads.
        let mut doc = Document::new_empty();
        for _ in 0..9000 {
            doc.add(Object::Null);
        }
        let mut fonts = Dict::new();
        let full = embed(&mut doc, &mut fonts, bytes, "Inter").unwrap();
        let objects = doc.scan_objects().count();
        assert_eq!(embed(&mut doc, &mut fonts, bytes, "Inter").unwrap(), full, "the same embedding is the same resource");
        let shaped = [("fi".to_string(), glyph('f')), ("é".to_string(), glyph('é'))];
        let first = embed_mapping(&mut doc, &mut fonts, bytes, "Inter", &shaped).unwrap();
        assert_ne!(first, full, "another mapping is another resource");
        assert_eq!(embed_mapping(&mut doc, &mut fonts, bytes, "Inter", &shaped).unwrap(), first);
        assert_eq!(embed_mapping(&mut doc, &mut fonts, bytes, "Inter", &shaped[..1]).map(|r| r != first), Ok(true), "a different mapping");
        assert_eq!(programs(&doc), 1, "one program for every mapping");
        assert_eq!(fonts.iter().count(), 3);
        assert!(doc.scan_objects().count() > objects);
        // Another writer may have compressed the streams since; the page still recognises them.
        for (r, o) in doc.scan_objects().collect::<Vec<_>>() {
            if let Object::Stream(st) = o.as_ref()
                && st.dict.get(b"Filter").is_none()
            {
                let data = st.decoded().unwrap();
                doc.set(r, Object::Stream(Stream::flate(st.dict.clone(), &data)));
            }
        }
        let objects = doc.scan_objects().count();
        assert_eq!(embed(&mut doc, &mut fonts, bytes, "Inter").unwrap(), full);
        assert_eq!(embed_mapping(&mut doc, &mut fonts, bytes, "Inter", &shaped).unwrap(), first);
        assert_eq!(doc.scan_objects().count(), objects, "nothing new is written");
        // A different program under the same name is not taken for this one.
        let other = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
        let mut more = fonts.clone();
        let medium = embed(&mut doc, &mut more, other, "Inter").unwrap();
        assert!(!fonts.contains(medium.as_bytes()), "{medium} is new");
        assert_eq!(programs(&doc), 2);
    }

    #[test]
    fn the_program_search_decodes_each_candidate_once_and_few_in_all() {
        let bytes = b"the program".as_slice();
        let decoy = |doc: &mut Document| {
            let mut d = Dict::new();
            d.set(b"PCFullFont".to_vec(), Object::Bool(true));
            d.set(b"Length1".to_vec(), Object::Int(bytes.len() as i64));
            doc.add(Object::Stream(Stream::flate(d, &[0; 4096])))
        };
        let resource = |doc: &mut Document, program: ObjRef| {
            let mut descriptor = Dict::new();
            descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
            let mut cid = Dict::new();
            cid.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
            let mut top = Dict::new();
            top.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid)]));
            doc.add(Object::Dict(top))
        };
        // 200 resources sharing one decoy program decode it once.
        let mut doc = Document::new_empty();
        let shared = decoy(&mut doc);
        let mut fonts = Dict::new();
        for i in 0..200 {
            let r = resource(&mut doc, shared);
            fonts.set(format!("PCFull{i}").into_bytes(), Object::Ref(r));
        }
        let mut programs = ProgramMatch::new(bytes);
        assert_eq!(existing_program(&doc, &fonts, &mut programs), None);
        assert_eq!(programs.decodes, 1);
        // 100 distinct decoys decode a bounded number of times.
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        for i in 0..100 {
            let program = decoy(&mut doc);
            let r = resource(&mut doc, program);
            fonts.set(format!("PCFull{i}").into_bytes(), Object::Ref(r));
        }
        let mut programs = ProgramMatch::new(bytes);
        assert_eq!(existing_program(&doc, &fonts, &mut programs), None);
        assert_eq!(programs.decodes, MAX_PROGRAM_DECODES);
    }

    #[test]
    fn the_full_program_is_written_compressed_and_another_page_reuses_it() {
        let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let mut doc = Document::new_empty();
        let mut first = Dict::new();
        embed(&mut doc, &mut first, bytes, "Inter").unwrap();
        let program = || {
            doc.scan_objects()
                .find_map(|(_, o)| match o.as_ref() {
                    Object::Stream(s) if s.dict.get(b"PCFullFont").is_some() => Some(s.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let stream = program();
        assert_eq!(stream.dict.name(b"Filter"), Some(b"FlateDecode".as_slice()));
        assert!(stream.raw.len() < bytes.len(), "{} of {} bytes", stream.raw.len(), bytes.len());
        assert_eq!(stream.decoded().unwrap(), bytes);
        // Another page's resources don't have it, so it is found among the document's objects.
        let objects = doc.scan_objects().count();
        let mut second = Dict::new();
        embed(&mut doc, &mut second, bytes, "Inter").unwrap();
        let programs = doc.scan_objects().filter(|(_, o)| matches!(o.as_ref(), Object::Stream(s) if s.dict.get(b"PCFullFont").is_some())).count();
        assert_eq!(programs, 1, "one program for both pages");
        assert!(doc.scan_objects().count() > objects, "the second page gets its own resource");
    }

    #[test]
    fn subset_truetype_creates_valid_compact_font() {
        let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let face = FontRef::new(bytes).unwrap();
        let gid_t = face.charmap().map('T').unwrap().to_u32() as u16;
        let gid_e = face.charmap().map('e').unwrap().to_u32() as u16;
        let (subset, mapping) = subset_truetype(bytes, &[gid_t, gid_e], "ABCDEF+Inter-Regular").unwrap();
        assert!(subset.len() < 10_000, "subset should be tiny, was {}", subset.len());
        assert!(mapping.contains_key(&gid_t));
        assert!(mapping.contains_key(&gid_e));
        let sub_face = FontRef::new(&subset).unwrap();
        assert_eq!(sub_face.charmap().map('T').unwrap().to_u32(), u32::from(*mapping.get(&gid_t).unwrap()));
        assert_eq!(sub_face.charmap().map('e').unwrap().to_u32(), u32::from(*mapping.get(&gid_e).unwrap()));
    }
}
