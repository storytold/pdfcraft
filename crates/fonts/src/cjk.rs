//! A CID font for the text layer of a page whose text is Chinese.
//!
//! The layer is invisible text (rendering mode 3), so it needs no glyph outlines: a Type0 font with
//! `Identity-H` encoding, a `/ToUnicode` map and the widths of the codes used is enough for a
//! reader to extract the text and place it. No font program is embedded, so the file's size does
//! not depend on the script.
//!
//! The font's dictionaries are direct, as the standard fonts a page's resources name are: only
//! the `/ToUnicode` map is an object of its own.

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};

/// The resource name a text layer refers to the font by.
pub const RESOURCE: &str = "PCCID";

/// The characters a text layer uses: each with the code it is written as and its advance in ems.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CjkFont {
    entries: Vec<(u16, char, f64)>,
}

impl CjkFont {
    /// A font for the given characters and their advances. Each character takes the next code,
    /// from 1: code 0 is the notdef glyph.
    pub fn new(chars: impl IntoIterator<Item = (char, f64)>) -> CjkFont {
        let mut entries: Vec<(u16, char, f64)> = Vec::new();
        for (ch, width) in chars {
            if entries.iter().any(|(_, c, _)| *c == ch) {
                continue;
            }
            let code = u16::try_from(entries.len() + 1).unwrap_or(u16::MAX);
            entries.push((code, ch, width));
        }
        CjkFont { entries }
    }

    /// The code a character is written as.
    pub fn code_of(&self, ch: char) -> Option<u16> {
        self.entries.iter().find(|(_, c, _)| *c == ch).map(|(code, _, _)| *code)
    }

    /// The characters, with their codes and advances.
    pub fn entries(&self) -> &[(u16, char, f64)] {
        &self.entries
    }
}

/// The font dictionary: a Type0 font with `Identity-H` encoding, a `/ToUnicode` map and the
/// widths of the codes used.
pub fn font_dict(doc: &mut Document, font: &CjkFont) -> Result<Dict, String> {
    let entries = font.entries();
    if entries.is_empty() {
        return Err("the font has no characters".into());
    }

    // The map from code back to text: a reader extracts the characters through it.
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    cmap.push_str(&format!("{} beginbfchar\n", entries.len()));
    for (code, ch, _) in entries {
        let mut units = [0u16; 2];
        let hex: String = ch.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
        cmap.push_str(&format!("<{code:04X}> <{hex}>\n"));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut cmap_dict = Dict::new();
    cmap_dict.set(b"Length".to_vec(), Object::Int(cmap.len() as i64));
    let cmap_ref = doc.add(Object::Stream(Stream::flate(cmap_dict, cmap.as_bytes())));

    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(RESOURCE));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(-200), Object::Int(1000), Object::Int(900)]));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    descriptor.set(b"Ascent".to_vec(), Object::Int(880));
    descriptor.set(b"Descent".to_vec(), Object::Int(-120));
    descriptor.set(b"CapHeight".to_vec(), Object::Int(880));
    descriptor.set(b"StemV".to_vec(), Object::Int(80));

    let mut widths = Vec::with_capacity(entries.len() * 2);
    for (code, _, width) in entries {
        widths.push(Object::Int(i64::from(*code)));
        widths.push(Object::Array(vec![Object::Real((width * 1000.0).round())]));
    }
    let mut descendant = Dict::new();
    descendant.set(b"Type".to_vec(), Object::name("Font"));
    descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    descendant.set(b"BaseFont".to_vec(), Object::name(RESOURCE));
    let mut info = Dict::new();
    info.set(b"Registry".to_vec(), Object::String(PdfString::literal(b"Adobe")));
    info.set(b"Ordering".to_vec(), Object::String(PdfString::literal(b"Identity")));
    info.set(b"Supplement".to_vec(), Object::Int(0));
    descendant.set(b"CIDSystemInfo".to_vec(), Object::Dict(info));
    descendant.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    descendant.set(b"DW".to_vec(), Object::Int(1000));
    descendant.set(b"W".to_vec(), Object::Array(widths));

    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name(RESOURCE));
    font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)]));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    Ok(font)
}
