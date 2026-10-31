//! What an embedded font program holds, so a font dictionary can be held up against it: its
//! glyphs and their names or CIDs, their advance widths, its built-in encoding and its cmap
//! subtables. PDF/UA's font conditions (ISO 14289-1 §7.21) compare the two.
//!
//! Programs are read with the parsers already in the tree, skrifa and read-fonts (sfnt, CFF and
//! Type 1), after decoding within [`MAX_PROGRAM`] bytes. Nothing here draws a glyph. A program that
//! doesn't decode or parse is [`Unreadable`]: what it holds is unknown, which is not the same as
//! holding nothing.

use std::cell::OnceCell;
use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, Object};
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::raw::ps::cs::CommandSink;
use skrifa::raw::ps::encoding::PredefinedEncoding;
use skrifa::raw::ps::type1::Type1Font;
use skrifa::raw::tables::cmap::PlatformId;
use skrifa::raw::types::{Fixed, GlyphId, Tag};
use skrifa::raw::{FontRef, TableProvider};

/// The most a font program may decode to; a larger one is left unread.
pub const MAX_PROGRAM: usize = 16 << 20;

/// Glyphs a program may have for its names or CIDs to be read (a CFF program holds at most 65 535).
const MAX_GLYPHS: u32 = 1 << 16;

/// The kind of program a font descriptor embeds (ISO 32000-1 Table 126).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `/FontFile`: a Type 1 program.
    Type1,
    /// `/FontFile2`: a TrueType program.
    TrueType,
    /// `/FontFile3` with `/Type1C` or `/CIDFontType0C`: a bare CFF program.
    Cff,
    /// `/FontFile3` with `/OpenType`: an OpenType program, with TrueType or CFF outlines.
    OpenType,
}

/// An embedded program that can't be read: it doesn't decode within [`MAX_PROGRAM`] bytes, or
/// doesn't parse as the kind of program its key says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// An embedded font program, decoded.
pub struct Program {
    format: Format,
    bytes: Vec<u8>,
    /// Type 1 programs are parsed once: the parse decrypts them.
    type1: Option<Type1Font>,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program").field("format", &self.format).field("bytes", &self.bytes.len()).finish()
    }
}

/// Whether `bytes` begin like an sfnt (TrueType or OpenType) program.
fn is_sfnt(bytes: &[u8]) -> bool {
    let tag = bytes.get(..4);
    [[0, 1, 0, 0], *b"OTTO", *b"true"].iter().any(|t| tag == Some(t.as_slice()))
}

impl Program {
    /// The program `descriptor` embeds: `Ok(None)` when it embeds none.
    pub fn embedded(doc: &Document, descriptor: &Dict) -> Result<Option<Program>, Unreadable> {
        let keys = [(b"FontFile".as_slice(), Format::Type1), (b"FontFile2".as_slice(), Format::TrueType), (b"FontFile3".as_slice(), Format::Cff)];
        let Some((key, declared)) = keys.into_iter().find(|(key, _)| descriptor.contains(key)) else { return Ok(None) };
        let object = descriptor.get(key).map(|o| doc.resolve(o));
        let Some(Object::Stream(stream)) = object.as_deref() else { return Err(Unreadable) };
        let bytes = stream.decoded_within(MAX_PROGRAM).map_err(|_| Unreadable)?;
        // `/FontFile3`'s `/Subtype` says CFF or OpenType; the bytes are believed over a wrong one.
        let format = match declared {
            Format::Type1 => Format::Type1,
            Format::TrueType => Format::TrueType,
            _ if is_sfnt(&bytes) => Format::OpenType,
            _ => Format::Cff,
        };
        Program::from_bytes(format, bytes).map(Some)
    }

    /// A program of `format` held in `bytes`.
    pub fn from_bytes(format: Format, bytes: Vec<u8>) -> Result<Program, Unreadable> {
        if bytes.len() > MAX_PROGRAM {
            return Err(Unreadable);
        }
        let type1 = match format {
            Format::Type1 => Some(Type1Font::new(&bytes).map_err(|_| Unreadable)?),
            _ => None,
        };
        let program = Program { format, bytes, type1 };
        if program.face().is_none() {
            return Err(Unreadable);
        }
        Ok(program)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// The program parsed, for looking things up in it. `None` only for a program that
    /// [`Program::from_bytes`] would have refused.
    pub fn face(&self) -> Option<Face<'_>> {
        match self.format {
            Format::Type1 => {
                let font = self.type1.as_ref()?;
                let num_glyphs = font.num_glyphs();
                if num_glyphs == 0 || num_glyphs > MAX_GLYPHS {
                    return None;
                }
                let mut names = HashMap::new();
                for (gid, name) in font.glyph_names() {
                    names.entry(name.to_owned()).or_insert(gid.to_u32());
                }
                Some(Face::new(self.format, None, None, Some(font), num_glyphs, units(font.upem()), names, HashMap::new()))
            }
            Format::Cff => Face::with_cff(self.format, None, CffFontRef::new(&self.bytes, 0, None).ok()?),
            Format::TrueType | Format::OpenType => {
                let font = FontRef::new(&self.bytes).ok()?;
                let num_glyphs = u32::from(font.maxp().ok()?.num_glyphs());
                let upem = font.head().ok()?.units_per_em();
                if num_glyphs == 0 || upem == 0 {
                    return None;
                }
                let cff = font.table_data(Tag::new(b"CFF ")).and_then(|d| CffFontRef::new(d.as_bytes(), 0, Some(i32::from(upem))).ok());
                match cff {
                    Some(cff) => Face::with_cff(self.format, Some(font), cff),
                    None => Some(Face::new(self.format, Some(font), None, None, num_glyphs, f64::from(upem), HashMap::new(), HashMap::new())),
                }
            }
        }
    }
}

/// Font units per em, as a divisor: 1000 when a program claims none.
fn units(upem: i32) -> f64 {
    if upem > 0 { f64::from(upem) } else { 1000.0 }
}

fn platform(p: PlatformId) -> u16 {
    match p {
        PlatformId::Unicode => 0,
        PlatformId::Macintosh => 1,
        PlatformId::ISO => 2,
        PlatformId::Windows => 3,
        PlatformId::Custom => 4,
        _ => u16::MAX,
    }
}

/// A charstring sink that keeps nothing: evaluating a charstring only for its advance width.
struct Nothing;

impl CommandSink for Nothing {
    fn move_to(&mut self, _: Fixed, _: Fixed) {}
    fn line_to(&mut self, _: Fixed, _: Fixed) {}
    fn curve_to(&mut self, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed) {}
    fn close(&mut self) {}
}

/// A parsed program, borrowed from its [`Program`].
pub struct Face<'a> {
    format: Format,
    sfnt: Option<FontRef<'a>>,
    cff: Option<CffFontRef<'a>>,
    type1: Option<&'a Type1Font>,
    num_glyphs: u32,
    /// Font units per em: glyph space to thousandths of an em.
    upem: f64,
    /// Glyph name → glyph id, the first glyph of a name (Type 1 and name-keyed CFF programs).
    names: HashMap<String, u32>,
    /// CID → glyph id (CID-keyed CFF programs).
    cids: HashMap<u32, u32>,
    /// The `post` table's names → glyph id, read the first time a name is looked up there.
    post: OnceCell<HashMap<String, u32>>,
}

impl<'a> Face<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        format: Format,
        sfnt: Option<FontRef<'a>>,
        cff: Option<CffFontRef<'a>>,
        type1: Option<&'a Type1Font>,
        num_glyphs: u32,
        upem: f64,
        names: HashMap<String, u32>,
        cids: HashMap<u32, u32>,
    ) -> Self {
        Face { format, sfnt, cff, type1, num_glyphs, upem, names, cids, post: OnceCell::new() }
    }

    /// A program whose glyphs are a CFF font's: bare, or the `CFF ` table of an OpenType program.
    fn with_cff(format: Format, sfnt: Option<FontRef<'a>>, cff: CffFontRef<'a>) -> Option<Self> {
        let num_glyphs = cff.num_glyphs();
        if num_glyphs == 0 || num_glyphs > MAX_GLYPHS {
            return None;
        }
        let (mut names, mut cids) = (HashMap::new(), HashMap::new());
        if let Some(charset) = cff.charset() {
            for (gid, sid) in charset.iter() {
                if cff.is_cid() {
                    cids.entry(u32::from(sid.to_u16())).or_insert(gid.to_u32());
                } else if let Some(name) = cff.string(sid).and_then(|s| std::str::from_utf8(s).ok()) {
                    names.entry(name.to_owned()).or_insert(gid.to_u32());
                }
            }
        }
        let upem = units(cff.upem());
        Some(Face::new(format, sfnt, Some(cff), None, num_glyphs, upem, names, cids))
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn num_glyphs(&self) -> u32 {
        self.num_glyphs
    }

    /// The glyph names a Type 1 or name-keyed CFF program defines; none for other programs.
    pub fn glyph_names(&self) -> impl Iterator<Item = &str> {
        self.names.keys().map(String::as_str)
    }

    /// The glyph a name selects: one of the program's glyph names for Type 1 and CFF outlines,
    /// a `post` table name for TrueType outlines.
    pub fn glyph_of_name(&self, name: &str) -> Option<u32> {
        if self.cff.is_none() && self.type1.is_none() {
            return self.post_names().get(name).copied();
        }
        self.names.get(name).copied()
    }

    /// The name of glyph `gid` in a Type 1 or name-keyed CFF program.
    pub fn glyph_name(&self, gid: u32) -> Option<String> {
        if let Some(font) = self.type1 {
            return font.glyph_name(GlyphId::new(gid)).map(str::to_owned);
        }
        let cff = self.cff.as_ref().filter(|c| !c.is_cid())?;
        let sid = cff.charset()?.string_id(GlyphId::new(gid)).ok()?;
        std::str::from_utf8(cff.string(sid)?).ok().map(str::to_owned)
    }

    /// The glyphs are CFF outlines: a bare CFF program, or an OpenType one with a `CFF ` table.
    pub fn cff_outlines(&self) -> bool {
        self.cff.is_some()
    }

    /// The glyphs are keyed by CID: a CID-keyed CFF program.
    pub fn cid_keyed(&self) -> bool {
        self.cff.as_ref().is_some_and(CffFontRef::is_cid)
    }

    /// The glyph of `cid` in a CID-keyed CFF program, through its charset.
    pub fn glyph_of_cid(&self, cid: u32) -> Option<u32> {
        self.cids.get(&cid).copied()
    }

    /// The CIDs a CID-keyed CFF program has glyphs for.
    pub fn cids(&self) -> impl Iterator<Item = u32> + '_ {
        self.cids.keys().copied()
    }

    /// The glyph the program's built-in encoding gives `code`: a Type 1 or name-keyed CFF
    /// program's own encoding, StandardEncoding when it declares none.
    pub fn builtin_glyph(&self, code: u8) -> Option<u32> {
        if let Some(font) = self.type1 {
            return match font.encoding() {
                Some(encoding) => encoding.map(code).map(|g| g.to_u32()),
                None => self.names.get(PredefinedEncoding::Standard.name(code)).copied(),
            };
        }
        let cff = self.cff.as_ref().filter(|c| !c.is_cid())?;
        cff.encoding()?.map(code).map(|g| g.to_u32())
    }

    /// The (platform, encoding) of each cmap subtable, in the table's order; empty when the
    /// program has no cmap table.
    pub fn cmaps(&self) -> Vec<(u16, u16)> {
        let Some(cmap) = self.sfnt.as_ref().and_then(|f| f.cmap().ok()) else { return Vec::new() };
        cmap.encoding_records().iter().map(|r| (platform(r.platform_id()), r.encoding_id())).collect()
    }

    /// The glyph the cmap subtable (`platform_id`, `encoding_id`) gives `code`: `None` when it
    /// gives none, or glyph 0.
    pub fn cmap_glyph(&self, platform_id: u16, encoding_id: u16, code: u32) -> Option<u32> {
        let cmap = self.sfnt.as_ref()?.cmap().ok()?;
        cmap.encoding_records()
            .iter()
            .filter(|r| platform(r.platform_id()) == platform_id && r.encoding_id() == encoding_id)
            .find_map(|r| r.subtable(cmap.offset_data()).ok()?.map_codepoint(code))
            .map(|g| g.to_u32())
            .filter(|g| *g != 0)
    }

    /// The advance width of glyph `gid` in thousandths of an em: from `hmtx` when the program has
    /// one, else from the glyph's charstring.
    pub fn advance(&self, gid: u32) -> Option<f64> {
        if gid >= self.num_glyphs {
            return None;
        }
        let glyph = GlyphId::new(gid);
        let units = if let Some(hmtx) = self.sfnt.as_ref().and_then(|f| f.hmtx().ok()) {
            f64::from(hmtx.advance(glyph)?)
        } else if let Some(cff) = &self.cff {
            let subfont = cff.subfont(cff.subfont_index(glyph)?, &[]).ok()?;
            f64::from(cff.evaluate_charstring(&subfont, glyph, &[], &mut Nothing).ok()??.to_f32())
        } else {
            f64::from(self.type1?.evaluate_charstring(glyph, &mut Nothing).ok()??.to_f32())
        };
        let width = units * 1000.0 / self.upem;
        width.is_finite().then_some(width)
    }

    /// Whether glyph `gid` has an outline: a non-empty `glyf` entry for TrueType outlines (a
    /// subsetter may keep glyph ids and empty the glyphs it drops); any glyph the program has
    /// for other outlines.
    pub fn has_outline(&self, gid: u32) -> bool {
        if gid >= self.num_glyphs {
            return false;
        }
        match (&self.sfnt, &self.cff) {
            (Some(font), None) => font.loca(None).ok().is_some_and(|loca| {
                let at = gid as usize;
                matches!((loca.get_raw(at), loca.get_raw(at.saturating_add(1))), (Some(start), Some(end)) if end > start)
            }),
            _ => true,
        }
    }

    fn post_names(&self) -> &HashMap<String, u32> {
        self.post.get_or_init(|| {
            let mut out = HashMap::new();
            if let Some(font) = &self.sfnt {
                for (gid, name) in skrifa::GlyphNames::new(font).iter().take(MAX_GLYPHS as usize) {
                    if !name.is_synthesized() {
                        out.entry(name.as_str().to_owned()).or_insert(gid.to_u32());
                    }
                }
            }
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

    /// `eexec`-encrypts `plain` (Adobe Type 1 Font Format §7.2) with seed `r`.
    pub(crate) fn encrypt(plain: &[u8], mut r: u16) -> Vec<u8> {
        plain
            .iter()
            .map(|&p| {
                let c = p ^ (r >> 8) as u8;
                r = (u16::from(c).wrapping_add(r)).wrapping_mul(52845).wrapping_add(22719);
                c
            })
            .collect()
    }

    /// A Type 1 charstring number (Type 1 Font Format §6.2).
    fn num(n: i32) -> Vec<u8> {
        match n {
            -107..=107 => vec![(n + 139) as u8],
            108..=1131 => vec![((n - 108) / 256 + 247) as u8, ((n - 108) % 256) as u8],
            _ => {
                let mut v = vec![255];
                v.extend_from_slice(&n.to_be_bytes());
                v
            }
        }
    }

    /// A Type 1 program whose glyphs are `.notdef` and the given names, each `width` wide and
    /// empty, with StandardEncoding.
    pub(crate) fn type1(names: &[(&str, i32)]) -> Vec<u8> {
        let mut glyphs = vec![(".notdef", 250)];
        glyphs.extend_from_slice(names);
        let mut private = b"dup /Private 8 dict dup begin /lenIV 4 def /Subrs 0 array\n".to_vec();
        private.extend_from_slice(format!("2 index /CharStrings {} dict dup begin\n", glyphs.len()).as_bytes());
        for (name, width) in glyphs {
            let mut cs = vec![0, 0, 0, 0];
            cs.extend(num(0));
            cs.extend(num(width));
            cs.extend([13, 14]);
            let cs = encrypt(&cs, 4330);
            private.extend_from_slice(format!("/{name} {} RD ", cs.len()).as_bytes());
            private.extend(cs);
            private.extend_from_slice(b" ND\n");
        }
        private.extend_from_slice(b"end end\nmark currentfile closefile\n");
        let mut plain = vec![0, 0, 0, 0];
        plain.extend(private);
        let mut out = b"%!PS-AdobeFont-1.0: Test 001\n/FontName /Test def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\ncurrentfile eexec\n".to_vec();
        out.extend(encrypt(&plain, 55665));
        out
    }

    #[test]
    fn an_sfnt_program_gives_its_glyphs_cmaps_and_widths() {
        let program = Program::from_bytes(Format::TrueType, INTER.to_vec()).expect("Inter parses");
        let face = program.face().expect("face");
        assert!(face.num_glyphs() > 100);
        assert!(face.cmaps().contains(&(3, 1)), "a Windows Unicode cmap: {:?}", face.cmaps());
        let a = face.cmap_glyph(3, 1, u32::from('A')).expect("A is mapped");
        let width = face.advance(a).expect("A has a width");
        assert!((500.0..800.0).contains(&width), "A is {width} thousandths wide");
        assert!(face.has_outline(a));
        assert_eq!(face.cmap_glyph(3, 1, 0x10_FFFF), None, "an unmapped code finds nothing");
        assert_eq!(face.advance(face.num_glyphs()), None, "no glyph past the last");
        assert!(!face.has_outline(face.num_glyphs()));
    }

    #[test]
    fn a_type1_program_gives_its_names_widths_and_builtin_encoding() {
        let program = Program::from_bytes(Format::Type1, type1(&[("A", 600), ("B", 700)])).expect("Type 1 parses");
        let face = program.face().expect("face");
        let mut names: Vec<&str> = face.glyph_names().collect();
        names.sort_unstable();
        assert_eq!(names, [".notdef", "A", "B"]);
        let b = face.glyph_of_name("B").expect("B");
        assert_eq!(face.glyph_name(b).as_deref(), Some("B"));
        assert_eq!(face.advance(b), Some(700.0));
        assert_eq!(face.builtin_glyph(b'A'), face.glyph_of_name("A"), "StandardEncoding puts A at 65");
        assert_eq!(face.glyph_of_name("C"), None);
        assert!(face.cmaps().is_empty() && !face.cid_keyed());
    }

    /// Damaged, truncated or mislabelled programs are unreadable, never a panic.
    #[test]
    fn damaged_programs_are_unreadable() {
        let t1 = type1(&[("A", 600)]);
        for format in [Format::Type1, Format::TrueType, Format::Cff, Format::OpenType] {
            for bytes in [
                Vec::new(),
                vec![0; 64],
                b"%!PS-AdobeFont-1.0\ncurrentfile eexec\n".to_vec(),
                INTER[..INTER.len() / 3].to_vec(),
                t1[..t1.len() / 2].to_vec(),
            ] {
                if let Ok(p) = Program::from_bytes(format, bytes) {
                    // Whatever parses must answer every question without panicking.
                    let face = p.face().expect("a program that parsed has a face");
                    for gid in [0, 1, face.num_glyphs(), u32::MAX] {
                        let _ = (face.advance(gid), face.has_outline(gid), face.glyph_name(gid));
                    }
                    let _ = (face.cmaps(), face.builtin_glyph(255), face.cmap_glyph(3, 1, 65));
                }
            }
        }
        // Every truncation of a Type 1 program, too.
        for cut in (0..t1.len()).step_by(7) {
            if let Ok(p) = Program::from_bytes(Format::Type1, t1[..cut].to_vec()) {
                let face = p.face().expect("face");
                let _ = (face.advance(0), face.builtin_glyph(65));
            }
        }
    }

    #[test]
    fn a_descriptor_without_a_program_embeds_none() {
        let doc = Document::new_empty();
        assert!(matches!(Program::embedded(&doc, &Dict::new()), Ok(None)));
        let mut d = Dict::new();
        d.set(b"FontFile2".to_vec(), Object::Int(3));
        assert_eq!(Program::embedded(&doc, &d).map(|p| p.is_some()), Err(Unreadable), "a program key that isn't a stream");
    }
}
