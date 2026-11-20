//! Pure COS wire builders for embedded Type0 / CIDFontType0 / CIDFontType2 fonts,
//! FontDescriptors, ToUnicode CMaps, and CID-to-GID CMap streams.
//!
//! No font-file parser (`skrifa`) or application asset policy lives here; callers supply
//! pre-validated metrics, glyph mappings, and program bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};

/// One mapped character/cluster in an embedded Type0 font.
pub(crate) struct CidMapping<'a> {
    pub cid: u16,
    pub glyph: u16,
    #[allow(dead_code)]
    pub width: f64,
    pub unicode: &'a str,
}

/// Scaled metrics and metadata for a PDF `/FontDescriptor` dictionary.
pub(crate) struct FontDescriptorSpec<'a> {
    pub font_name: &'a str,
    pub family: &'a str,
    pub italic_angle: f64,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub bbox: [f64; 4],
    pub cff: bool,
    pub program: Object,
}

/// Add an uncompressed raw COS stream to `doc` and return its indirect reference.
pub(crate) fn raw_stream(doc: &mut Document, bytes: Vec<u8>) -> Object {
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), bytes))))
}

/// Build a compressed full font program stream (`FontFile2` or `FontFile3` for CFF/OpenType).
pub(crate) fn font_program_stream(doc: &mut Document, bytes: &[u8], cff: bool) -> Object {
    let mut d = Dict::new();
    d.set(b"Length1".to_vec(), Object::Int(bytes.len() as i64));
    if cff {
        d.set(b"Subtype".to_vec(), Object::name("OpenType"));
    }
    d.set(b"PCFullFont".to_vec(), Object::Bool(true));
    Object::Ref(doc.add(Object::Stream(Stream::flate(d, bytes))))
}

/// Build the `/ToUnicode` CMap stream (`PCUnicode`) mapping 2-byte CIDs to UTF-16BE hex strings.
pub(crate) fn build_to_unicode_cmap(mappings: &[CidMapping<'_>]) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PCUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in mappings.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for m in chunk {
            let hex: String = m.unicode.encode_utf16().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{:04X}> <{hex}>", m.cid);
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// Build the `/Encoding` CMap stream (`PCGlyphs`) mapping 2-byte CIDs to CFF glyph IDs.
pub(crate) fn build_cid_encoding_cmap(mappings: &[CidMapping<'_>]) -> String {
    let mut cid_encoding = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PCGlyphs def\n/CMapType 1 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in mappings.chunks(100) {
        let _ = writeln!(cid_encoding, "{} begincidchar", chunk.len());
        for m in chunk {
            let _ = writeln!(cid_encoding, "<{:04X}> {}", m.cid, m.glyph);
        }
        cid_encoding.push_str("endcidchar\n");
    }
    cid_encoding.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cid_encoding
}

/// Build a PDF `/FontDescriptor` dictionary.
pub(crate) fn build_font_descriptor(spec: &FontDescriptorSpec<'_>) -> Dict {
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(spec.font_name));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(spec.family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Real(spec.italic_angle));
    descriptor.set(b"Ascent".to_vec(), Object::Real(spec.ascent));
    descriptor.set(b"Descent".to_vec(), Object::Real(spec.descent));
    descriptor.set(b"CapHeight".to_vec(), Object::Real(spec.cap_height));
    descriptor.set(b"StemV".to_vec(), Object::Int(80));
    descriptor.set(b"FontBBox".to_vec(), Object::Array(spec.bbox.map(Object::Real).to_vec()));
    descriptor.set(if spec.cff { b"FontFile3".to_vec() } else { b"FontFile2".to_vec() }, spec.program.clone());
    descriptor
}

/// Build a descendant `CIDFontType0` or `CIDFontType2` dictionary.
pub(crate) fn build_cid_font_dict(
    doc: &mut Document,
    name: &str,
    descriptor: Object,
    cff: bool,
    cff_widths: BTreeMap<u16, f64>,
    gids: Vec<u8>,
    widths: Vec<Object>,
) -> Dict {
    let mut system = Dict::new();
    system.set(b"Registry".to_vec(), Object::String(PdfString::literal(b"Adobe".to_vec())));
    system.set(b"Ordering".to_vec(), Object::String(PdfString::literal(b"Identity".to_vec())));
    system.set(b"Supplement".to_vec(), Object::Int(0));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name(if cff { "CIDFontType0" } else { "CIDFontType2" }));
    cid.set(b"BaseFont".to_vec(), Object::name(name));
    cid.set(b"CIDSystemInfo".to_vec(), Object::Dict(system));
    cid.set(b"FontDescriptor".to_vec(), descriptor);
    if cff {
        cid.set(
            b"W".to_vec(),
            Object::Array(cff_widths.into_iter().flat_map(|(gid, w)| [Object::Int(i64::from(gid)), Object::Array(vec![Object::Real(w)])]).collect()),
        );
    } else {
        cid.set(b"CIDToGIDMap".to_vec(), raw_stream(doc, gids));
        cid.set(b"W".to_vec(), Object::Array(vec![Object::Int(1), Object::Array(widths)]));
    }
    cid
}

/// Build the top-level `Type0` font dictionary.
pub(crate) fn build_type0_font_dict(doc: &mut Document, name: &str, descendant: Object, cid_encoding: Option<Vec<u8>>, to_unicode: Vec<u8>) -> Dict {
    let mut top = Dict::new();
    top.set(b"Type".to_vec(), Object::name("Font"));
    top.set(b"Subtype".to_vec(), Object::name("Type0"));
    top.set(b"BaseFont".to_vec(), Object::name(name));
    top.set(
        b"Encoding".to_vec(),
        match cid_encoding {
            Some(bytes) => raw_stream(doc, bytes),
            None => Object::name("Identity-H"),
        },
    );
    top.set(b"DescendantFonts".to_vec(), Object::Array(vec![descendant]));
    top.set(b"ToUnicode".to_vec(), raw_stream(doc, to_unicode));
    top
}
