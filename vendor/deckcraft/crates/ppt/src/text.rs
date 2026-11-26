//! Text in `.ppt` ([MS-PPT] §2.9): characters in a `TextCharsAtom` (UTF-16) or `TextBytesAtom`
//! (Latin-1), formatting as runs of paragraph exceptions (`TextPFException`) and character
//! exceptions (`TextCFException`) over the master's per-level text styles.

use crate::doc::rt;
use crate::record::{Cursor, Rec};

/// A colour as text formatting stores it: an RGB value or an index into the colour scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextColor {
    Rgb(u8, u8, u8),
    Scheme(u8),
}

fn color_index(c: &mut Cursor) -> Option<Option<TextColor>> {
    let (r, g, b, i) = (c.u8()?, c.u8()?, c.u8()?, c.u8()?);
    Some(match i {
        0xFE => Some(TextColor::Rgb(r, g, b)),
        0..=7 => Some(TextColor::Scheme(i)),
        _ => None,
    })
}

/// Paragraph formatting; `None` fields are not set at this level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pf {
    pub has_bullet: Option<bool>,
    pub bullet_has_font: Option<bool>,
    pub bullet_has_color: Option<bool>,
    pub bullet_has_size: Option<bool>,
    pub bullet_char: Option<u16>,
    pub bullet_font: Option<u16>,
    /// Positive: percent of the text size; negative: absolute size in points.
    pub bullet_size: Option<i16>,
    pub bullet_color: Option<TextColor>,
    /// 0 left, 1 centre, 2 right, 3 justify, 4 distributed, 5 Thai distributed, 6 justify low.
    pub align: Option<u16>,
    /// Positive: percent of a line; negative: master units (−8 per point).
    pub line_spacing: Option<i16>,
    pub space_before: Option<i16>,
    pub space_after: Option<i16>,
    /// Master units from the text box's left edge.
    pub left_margin: Option<u16>,
    pub indent: Option<u16>,
    pub default_tab: Option<u16>,
    pub tabs: Option<Vec<(i16, u16)>>,
}

impl Pf {
    /// Overlay `o`'s set fields on this one.
    pub fn apply(&mut self, o: &Pf) {
        macro_rules! over {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f.clone_from(&o.$f); } )* };
        }
        over!(
            has_bullet,
            bullet_has_font,
            bullet_has_color,
            bullet_has_size,
            bullet_char,
            bullet_font,
            bullet_size,
            bullet_color,
            align,
            line_spacing,
            space_before,
            space_after,
            left_margin,
            indent,
            default_tab,
            tabs
        );
    }
}

/// Character formatting; `None` fields are not set at this level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cf {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub shadow: Option<bool>,
    pub font: Option<u16>,
    pub ea_font: Option<u16>,
    pub symbol_font: Option<u16>,
    /// Points.
    pub size: Option<u16>,
    pub color: Option<TextColor>,
    /// Superscript (positive) or subscript (negative) offset, percent of the size.
    pub position: Option<i16>,
    /// `pp9rt`: the index of the paragraph's PowerPoint 2000 formatting (numbering) in the text's
    /// `StyleTextProp9Atom`.
    pub pp9rt: Option<u8>,
}

impl Cf {
    pub fn apply(&mut self, o: &Cf) {
        macro_rules! over {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        over!(bold, italic, underline, shadow, font, ea_font, symbol_font, size, color, position, pp9rt);
    }
}

fn bit(m: u32, b: u32) -> bool {
    m & (1 << b) != 0
}

/// A `TextPFException`.
pub fn read_pf(c: &mut Cursor) -> Option<Pf> {
    let m = c.u32()?;
    let mut pf = Pf::default();
    if m & 0xF != 0 {
        let flags = c.u16()?;
        for (b, slot) in [(0, &mut pf.has_bullet), (1, &mut pf.bullet_has_font), (2, &mut pf.bullet_has_color), (3, &mut pf.bullet_has_size)] {
            if bit(m, b) {
                *slot = Some(flags & (1 << b) != 0);
            }
        }
    }
    if bit(m, 7) {
        pf.bullet_char = Some(c.u16()?);
    }
    if bit(m, 4) {
        pf.bullet_font = Some(c.u16()?);
    }
    if bit(m, 6) {
        pf.bullet_size = Some(c.i16()?);
    }
    if bit(m, 5) {
        pf.bullet_color = color_index(c)?;
    }
    if bit(m, 11) {
        pf.align = Some(c.u16()?);
    }
    if bit(m, 12) {
        pf.line_spacing = Some(c.i16()?);
    }
    if bit(m, 13) {
        pf.space_before = Some(c.i16()?);
    }
    if bit(m, 14) {
        pf.space_after = Some(c.i16()?);
    }
    if bit(m, 8) {
        pf.left_margin = Some(c.u16()?);
    }
    if bit(m, 10) {
        pf.indent = Some(c.u16()?);
    }
    if bit(m, 15) {
        pf.default_tab = Some(c.u16()?);
    }
    if bit(m, 20) {
        pf.tabs = Some(read_tabs(c)?);
    }
    if bit(m, 16) {
        c.u16()?; // fontAlign
    }
    if m & (0x7 << 17) != 0 {
        c.u16()?; // wrapFlags
    }
    if bit(m, 21) {
        c.u16()?; // textDirection
    }
    Some(pf)
}

fn read_tabs(c: &mut Cursor) -> Option<Vec<(i16, u16)>> {
    let n = c.u16()?;
    let mut v = Vec::with_capacity(usize::from(n).min(64));
    for _ in 0..n {
        v.push((c.i16()?, c.u16()?));
    }
    Some(v)
}

/// A `TextCFException`.
pub fn read_cf(c: &mut Cursor) -> Option<Cf> {
    let m = c.u32()?;
    let mut cf = Cf::default();
    // fontStyle is present when any of bold, italic, underline, shadow, fehint, kumi, emboss or
    // fHasStyle is in the mask.
    if m & 0x3EB7 != 0 {
        let s = c.u16()?;
        // Emboss (bit 9) has no counterpart in the model; docs/gaps.md lists it.
        for (b, slot) in [(0, &mut cf.bold), (1, &mut cf.italic), (2, &mut cf.underline), (4, &mut cf.shadow)] {
            if bit(m, b) {
                *slot = Some(s & (1 << b) != 0);
            }
        }
        // fHasStyle (bits 10–13) says pp9rt, the same four bits of fontStyle, is given.
        if m & 0x3C00 != 0 {
            cf.pp9rt = Some(((s >> 10) & 0xF) as u8);
        }
    }
    if bit(m, 16) {
        cf.font = Some(c.u16()?);
    }
    if bit(m, 21) {
        cf.ea_font = Some(c.u16()?);
    }
    if bit(m, 22) {
        c.u16()?; // ansiFontRef
    }
    if bit(m, 23) {
        cf.symbol_font = Some(c.u16()?);
    }
    if bit(m, 17) {
        cf.size = Some(c.u16()?);
    }
    if bit(m, 18) {
        cf.color = color_index(c)?;
    }
    if bit(m, 19) {
        cf.position = Some(c.i16()?);
    }
    // The fields PowerPoint 2002 and later add are not used, but they must be read past: the next
    // run's count and mask follow them, so stopping short garbles every later run's formatting.
    if bit(m, 20) {
        c.u32()?; // pp10runid (4 bits) and unused (28 bits)
    }
    if bit(m, 24) {
        c.u16()?; // newEAFontRef
    }
    if bit(m, 25) {
        c.u16()?; // csFontRef
    }
    if bit(m, 26) {
        c.u32()?; // pp11ext
    }
    Some(cf)
}

/// A paragraph's PowerPoint 2000 formatting (`TextPFException9`): its automatic numbering.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pf9 {
    /// `fBulletHasAutoNumber` with `bulletAutoNumberScheme`: the scheme and the first number.
    pub auto_number: Option<(u16, i16)>,
}

/// The entries of a `StyleTextProp9Atom` ([MS-PPT] §2.9.33): each a `TextPFException9`, a
/// `TextCFException9` and a `TextSIException`, indexed by the runs' `pp9rt`. Reading stops at an
/// entry with a field this reader can't size, keeping the entries before it.
pub fn read_style9(b: &[u8]) -> Vec<Pf9> {
    let mut c = Cursor::new(b);
    let mut out = vec![];
    // pp9rt has four bits, so sixteen entries are all a run can name.
    while out.len() < 16 && c.remaining() > 0 {
        let Some(pf9) = read_pf9(&mut c) else { break };
        out.push(pf9);
        // TextCFException9: the extension fields of a character mask.
        let Some(m) = c.u32() else { break };
        if m & !((1 << 20) | (1 << 24) | (1 << 25) | (1 << 26)) != 0 {
            break;
        }
        let ok = (!bit(m, 20) || c.u32().is_some())
            && (!bit(m, 24) || c.u16().is_some())
            && (!bit(m, 25) || c.u16().is_some())
            && (!bit(m, 26) || c.u32().is_some());
        // TextSIException: spelling, language, alternate language, bidi, pp10 extension, smart tags.
        if !ok || read_si(&mut c).is_none() {
            break;
        }
    }
    out
}

fn read_pf9(c: &mut Cursor) -> Option<Pf9> {
    let m = c.u32()?;
    if bit(m, 23) {
        c.i16()?; // bulletBlipRef
    }
    let has_auto = if bit(m, 25) { Some(c.u16()? != 0) } else { None };
    let scheme = if bit(m, 24) { Some((c.u16()?, c.i16()?)) } else { None };
    if bit(m, 26) {
        c.u32()?; // pp10 extension
    }
    Some(Pf9 { auto_number: if has_auto == Some(true) { Some(scheme.unwrap_or((3, 1))) } else { None } })
}

fn read_si(c: &mut Cursor) -> Option<()> {
    let m = c.u32()?;
    if m & !0x027F != 0 {
        return None;
    }
    for b in [0, 1, 2, 6] {
        if bit(m, b) {
            c.u16()?;
        }
    }
    if bit(m, 5) {
        c.u32()?; // pp10runid and reserved
    }
    if bit(m, 9) {
        let n = c.u32()?;
        for _ in 0..n {
            c.u32()?;
        }
    }
    Some(())
}

/// The DrawingML numbering scheme of a `TextAutoNumberSchemeEnum` value.
pub fn auto_number_scheme(v: u16) -> &'static str {
    match v {
        0 => "alphaLcPeriod",
        1 => "alphaUcPeriod",
        2 => "arabicParenR",
        4 => "romanLcParenBoth",
        5 => "romanLcParenR",
        6 => "romanLcPeriod",
        7 => "romanUcPeriod",
        8 => "alphaLcParenBoth",
        9 => "alphaLcParenR",
        10 => "alphaUcParenBoth",
        11 => "alphaUcParenR",
        12 => "arabicParenBoth",
        13 => "arabicPlain",
        14 => "romanUcParenBoth",
        15 => "romanUcParenR",
        _ => "arabicPeriod",
    }
}

/// Text types (`TextHeaderAtom.textType`).
pub mod ty {
    pub const TITLE: u32 = 0;
    pub const BODY: u32 = 1;
    pub const NOTES: u32 = 2;
    pub const OTHER: u32 = 4;
    pub const CENTER_BODY: u32 = 5;
    pub const CENTER_TITLE: u32 = 6;
    pub const HALF_BODY: u32 = 7;
    pub const QUARTER_BODY: u32 = 8;
}

/// One master text style: up to five levels of paragraph and character formatting.
#[derive(Clone, Debug, Default)]
pub struct LevelStyles {
    pub levels: Vec<(Pf, Cf)>,
}

/// A `TextMasterStyleAtom` body.
pub fn read_master_style(r: &Rec) -> LevelStyles {
    let mut c = Cursor::new(r.data);
    let mut out = LevelStyles::default();
    let Some(n) = c.u16() else { return out };
    for _ in 0..n.min(5) {
        // From the centre-body type on, each level names itself.
        if u32::from(r.instance) >= ty::CENTER_BODY && c.u16().is_none() {
            break;
        }
        let Some(pf) = read_pf(&mut c) else { break };
        let Some(cf) = read_cf(&mut c) else { break };
        out.levels.push((pf, cf));
    }
    out
}

/// The text styles in effect for a master: the document defaults plus the master's styles by text
/// type.
#[derive(Clone, Debug, Default)]
pub struct StyleSheet {
    pub default_pf: Pf,
    pub default_cf: Cf,
    /// Indexed by text type (0–8).
    pub types: Vec<Option<LevelStyles>>,
}

impl StyleSheet {
    /// Read the defaults of the document environment and, over them, a master's styles.
    pub fn new(environment: Option<Rec>, master: Option<&Rec>) -> Self {
        let mut s = StyleSheet { types: vec![None; 9], ..Default::default() };
        if let Some(env) = environment {
            if let Some(pf) = env.child(rt::TEXT_PF_EXCEPTION).and_then(|r| read_pf(&mut Cursor { b: r.data, at: 2 })) {
                s.default_pf = pf;
            }
            if let Some(cf) = env.child(rt::TEXT_CF_EXCEPTION).and_then(|r| read_cf(&mut Cursor::new(r.data))) {
                s.default_cf = cf;
            }
            for r in env.children_of(rt::TEXT_MASTER_STYLE) {
                if let Some(slot) = s.types.get_mut(usize::from(r.instance)) {
                    *slot = Some(read_master_style(&r));
                }
            }
        }
        if let Some(m) = master {
            for r in m.children_of(rt::TEXT_MASTER_STYLE) {
                if let Some(slot) = s.types.get_mut(usize::from(r.instance)) {
                    *slot = Some(read_master_style(&r));
                }
            }
        }
        s
    }

    /// The formatting of text type `ty` at outline level `level`: defaults, then the base type's
    /// level 1 and level, then this type's.
    pub fn resolve(&self, ty: u32, level: u8) -> (Pf, Cf) {
        let (mut pf, mut cf) = (self.default_pf.clone(), self.default_cf.clone());
        let base = match ty {
            ty::CENTER_TITLE => Some(ty::TITLE),
            ty::CENTER_BODY | ty::HALF_BODY | ty::QUARTER_BODY => Some(ty::BODY),
            _ => None,
        };
        for t in base.into_iter().chain(std::iter::once(ty)) {
            let Some(Some(st)) = self.types.get(t as usize) else { continue };
            for l in [0usize, usize::from(level)] {
                if let Some((p, c)) = st.levels.get(l) {
                    pf.apply(p);
                    cf.apply(c);
                }
                if level == 0 {
                    break;
                }
            }
        }
        (pf, cf)
    }
}

/// A field inside text: its character position and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    SlideNumber,
    DateTime,
}

/// A block of text with its formatting runs.
#[derive(Clone, Debug, Default)]
pub struct TextBlock {
    pub text_type: u32,
    pub chars: Vec<u16>,
    /// (character count, indent level, paragraph exception).
    pub para_runs: Vec<(u32, u8, Pf)>,
    /// (character count, character exception).
    pub char_runs: Vec<(u32, Cf)>,
    /// Ruler overrides per level: (left margin, indent) in master units.
    pub ruler: [(Option<u16>, Option<u16>); 5],
    pub fields: Vec<(u32, FieldKind)>,
    /// An `OutlineTextRefAtom`: the text is the n-th outline block of the slide list entry.
    pub outline_ref: Option<u32>,
    /// The slide list's outline block this text was read from.
    pub outline_index: Option<usize>,
}

/// Read a text block from a run of records starting at its `TextHeaderAtom` (inside a client text
/// box, or in the slide list). Stops at the next header or slide persist atom. A client text box
/// may hold only an `OutlineTextRefAtom` ([MS-PPT] OfficeArtClientTextbox), with no header: that
/// is still a block, so the caller can follow the reference.
pub fn read_block<'a>(records: impl Iterator<Item = Rec<'a>>) -> Option<TextBlock> {
    let mut b = TextBlock::default();
    let mut started = false;
    let mut style: Option<Rec> = None;
    for r in records {
        match r.kind {
            rt::TEXT_HEADER if started => break,
            rt::SLIDE_PERSIST_ATOM => break,
            rt::TEXT_HEADER => {
                started = true;
                b.text_type = r.u32(0).unwrap_or(ty::OTHER);
            }
            rt::TEXT_CHARS => b.chars = r.data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect(),
            rt::TEXT_BYTES => b.chars = r.data.iter().map(|&c| u16::from(c)).collect(),
            rt::STYLE_TEXT_PROP => style = Some(r),
            rt::TEXT_RULER => b.ruler = read_ruler(&r),
            rt::OUTLINE_TEXT_REF => b.outline_ref = r.u32(0),
            rt::SLIDE_NUMBER_MC => b.fields.extend(r.u32(0).map(|p| (p, FieldKind::SlideNumber))),
            rt::DATETIME_MC | rt::GENERIC_DATE_MC => b.fields.extend(r.u32(0).map(|p| (p, FieldKind::DateTime))),
            _ => {}
        }
    }
    if !started && b.outline_ref.is_none() {
        return None;
    }
    if let Some(st) = style {
        // Text past u32::MAX characters can't be addressed by the runs; saturate rather than wrap.
        read_style_runs(&st, u32::try_from(b.chars.len()).unwrap_or(u32::MAX).saturating_add(1), &mut b);
    }
    Some(b)
}

/// The paragraph runs then the character runs of a `StyleTextPropAtom`; each set covers the text
/// plus one (the final paragraph mark).
fn read_style_runs(r: &Rec, total: u32, b: &mut TextBlock) {
    let mut c = Cursor::new(r.data);
    let mut covered = 0u32;
    while covered < total {
        let Some(n) = c.u32() else { return };
        let Some(level) = c.u16() else { return };
        let Some(pf) = read_pf(&mut c) else { return };
        b.para_runs.push((n, level.min(8) as u8, pf));
        covered = covered.saturating_add(n.max(1));
    }
    covered = 0;
    while covered < total {
        let Some(n) = c.u32() else { return };
        let Some(cf) = read_cf(&mut c) else { return };
        b.char_runs.push((n, cf));
        covered = covered.saturating_add(n.max(1));
    }
}

fn read_ruler(r: &Rec) -> [(Option<u16>, Option<u16>); 5] {
    let mut out = [(None, None); 5];
    let mut c = Cursor::new(r.data);
    let Some(m) = c.u32() else { return out };
    if bit(m, 1) && c.u16().is_none() {
        return out;
    }
    if bit(m, 0) && c.u16().is_none() {
        return out;
    }
    if bit(m, 2) && read_tabs(&mut c).is_none() {
        return out;
    }
    for (l, slot) in out.iter_mut().enumerate() {
        let l = l as u32;
        if bit(m, 3 + l) {
            slot.0 = c.u16();
        }
        if bit(m, 8 + l) {
            slot.1 = c.u16();
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::record::build::*;

    /// A PF exception with alignment and bullets on or off.
    pub fn pf_bytes(align: u16, bullet: bool) -> Vec<u8> {
        let mut v = (0x0000_0801u32).to_le_bytes().to_vec(); // hasBullet + align
        v.extend_from_slice(&u16::from(bullet).to_le_bytes());
        v.extend_from_slice(&align.to_le_bytes());
        v
    }

    /// A CF exception with bold, a font, a size and an RGB colour.
    pub fn cf_bytes(bold: bool, font: u16, size: u16, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = (0x0007_0001u32).to_le_bytes().to_vec(); // bold + typeface + size + color
        v.extend_from_slice(&u16::from(bold).to_le_bytes());
        v.extend_from_slice(&font.to_le_bytes());
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 0xFE]);
        v
    }

    /// The `StyleTextProp9Atom` entries as PowerPoint writes them: three, the second numbered "1."
    /// from 1. Short or unknown data keeps the entries read before it.
    #[test]
    fn reads_numbering_entries() {
        let mut b = vec![0u8; 12];
        b.extend_from_slice(&[0, 0, 0, 3, 1, 0, 3, 0, 1, 0]);
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&[0; 12]);
        let e = read_style9(&b);
        assert_eq!(e.iter().map(|p| p.auto_number).collect::<Vec<_>>(), [None, Some((3, 1)), None]);
        assert_eq!(read_style9(b.get(..20).unwrap()).len(), 1);
        assert_eq!(read_style9(b.get(..22).unwrap()).len(), 2);
        assert_eq!(read_style9(&[0xFF; 64]).len(), 1);
        assert!(read_style9(&[]).is_empty());
        assert_eq!(auto_number_scheme(3), "arabicPeriod");
        assert_eq!(auto_number_scheme(0), "alphaLcPeriod");
    }

    #[test]
    fn reads_exceptions_in_mask_order() {
        let pf = read_pf(&mut Cursor::new(&pf_bytes(1, true))).unwrap();
        assert_eq!(pf.has_bullet, Some(true));
        assert_eq!(pf.align, Some(1));
        assert_eq!(pf.bullet_has_font, None);
        let cf = read_cf(&mut Cursor::new(&cf_bytes(true, 3, 28, [1, 2, 3]))).unwrap();
        assert_eq!((cf.bold, cf.italic, cf.font, cf.size, cf.color), (Some(true), None, Some(3), Some(28), Some(TextColor::Rgb(1, 2, 3))));
        // A mask promising more than the bytes hold reads nothing.
        assert!(read_cf(&mut Cursor::new(&0x0007_0000u32.to_le_bytes())).is_none());
    }

    #[test]
    fn reads_a_text_block_with_runs() {
        let chars: Vec<u8> = "Hi\rthere".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut style = vec![];
        for (n, lvl) in [(3u32, 0u16), (6, 1)] {
            style.extend_from_slice(&n.to_le_bytes());
            style.extend_from_slice(&lvl.to_le_bytes());
            style.extend_from_slice(&pf_bytes(0, lvl == 1));
        }
        for n in [2u32, 7] {
            style.extend_from_slice(&n.to_le_bytes());
            style.extend_from_slice(&cf_bytes(n == 2, 0, 20, [0, 0, 0]));
        }
        let recs = [
            atom(rt::TEXT_HEADER, 0, &1u32.to_le_bytes()),
            atom(rt::TEXT_CHARS, 0, &chars),
            atom(rt::STYLE_TEXT_PROP, 0, &style),
            atom(rt::TEXT_HEADER, 0, &0u32.to_le_bytes()),
        ]
        .concat();
        let b = read_block(crate::record::Records::new(&recs)).unwrap();
        assert_eq!(b.text_type, ty::BODY);
        assert_eq!(String::from_utf16_lossy(&b.chars), "Hi\rthere");
        assert_eq!(b.para_runs.len(), 2);
        assert_eq!(b.para_runs[1].1, 1);
        assert_eq!(b.char_runs.len(), 2);
        assert_eq!(b.char_runs[0].1.bold, Some(true));
    }

    /// A CF exception with only the fields PowerPoint 2002 and later add after `position`:
    /// pp10runid (bit 20), newEAFontRef (bit 24), csFontRef (bit 25) and pp11ext (bit 26).
    fn cf_later_fields() -> Vec<u8> {
        let mut v = (1u32 << 20 | 1 << 24 | 1 << 25 | 1 << 26).to_le_bytes().to_vec();
        v.extend_from_slice(&0x0000_0003u32.to_le_bytes()); // pp10runid + unused
        v.extend_from_slice(&7u16.to_le_bytes()); // newEAFontRef
        v.extend_from_slice(&8u16.to_le_bytes()); // csFontRef
        v.extend_from_slice(&0u32.to_le_bytes()); // pp11ext
        v
    }

    #[test]
    fn later_character_fields_are_read_past() {
        let bytes = cf_later_fields();
        let mut c = Cursor::new(&bytes);
        assert!(read_cf(&mut c).is_some());
        assert!(c.u8().is_none(), "the cursor stopped inside the exception");

        // A run carrying those fields, then a bold 18 pt run: the second must read as written.
        let chars: Vec<u8> = "abcd".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut style = vec![];
        style.extend_from_slice(&5u32.to_le_bytes());
        style.extend_from_slice(&0u16.to_le_bytes());
        style.extend_from_slice(&pf_bytes(0, false));
        style.extend_from_slice(&2u32.to_le_bytes());
        style.extend_from_slice(&cf_later_fields());
        style.extend_from_slice(&3u32.to_le_bytes());
        style.extend_from_slice(&cf_bytes(true, 1, 18, [0, 0, 0]));
        let recs = [atom(rt::TEXT_HEADER, 0, &1u32.to_le_bytes()), atom(rt::TEXT_CHARS, 0, &chars), atom(rt::STYLE_TEXT_PROP, 0, &style)].concat();
        let b = read_block(crate::record::Records::new(&recs)).unwrap();
        assert_eq!(b.char_runs.len(), 2, "{:?}", b.char_runs);
        let (n, cf) = &b.char_runs[1];
        assert_eq!((*n, cf.bold, cf.font, cf.size), (3, Some(true), Some(1), Some(18)));
    }

    #[test]
    fn styles_inherit_from_level_one_and_the_base_type() {
        let mut s = StyleSheet { types: vec![None; 9], ..Default::default() };
        let lvl = |size: u16, align: u16| (Pf { align: Some(align), ..Default::default() }, Cf { size: Some(size), ..Default::default() });
        s.types[ty::BODY as usize] = Some(LevelStyles { levels: vec![lvl(32, 0), (Pf::default(), Cf { size: Some(28), ..Default::default() })] });
        s.types[ty::CENTER_BODY as usize] = Some(LevelStyles { levels: vec![(Pf { align: Some(1), ..Default::default() }, Cf::default())] });
        let (pf, cf) = s.resolve(ty::CENTER_BODY, 1);
        assert_eq!(cf.size, Some(28));
        assert_eq!(pf.align, Some(1));
        let (pf, cf) = s.resolve(ty::BODY, 0);
        assert_eq!((pf.align, cf.size), (Some(0), Some(32)));
    }
}
