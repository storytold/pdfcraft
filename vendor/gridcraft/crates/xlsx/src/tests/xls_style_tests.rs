//! Legacy `.xls` (BIFF8) cell formatting: Font, XF, Palette and Style records. Every workbook is
//! assembled here from the [MS-XLS] structures; no Excel-produced file is used.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::style::Protection;
use gridcraft_model::{BorderLine, BorderStyle, Color, Fill, HAlign, PatternType, Style, StyleId, Underline, VAlign, VertAlign, Workbook};

use super::xls_tests::compound;
use crate::tables::INDEXED;
use crate::{read_xls, read_xlsx, write_xlsx};

fn rec(id: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = id.to_le_bytes().to_vec();
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn bof(dt: u16) -> Vec<u8> {
    let mut p = u16s(&[0x0600, dt, 0x0DBB, 0x07CC]);
    p.extend_from_slice(&[0; 8]);
    rec(0x0809, &p)
}

fn eof() -> Vec<u8> {
    rec(0x000A, &[])
}

/// Font: height in twips, flags, colour, weight, script, underline, then a UTF-16 name.
fn font(twips: u16, flags: u16, icv: u16, weight: u16, script: u16, underline: u8, name: &str) -> Vec<u8> {
    let mut p = u16s(&[twips, flags, icv, weight, script]);
    p.extend_from_slice(&[underline, 0, 0, 0]);
    p.push(name.encode_utf16().count() as u8);
    p.push(1);
    p.extend(u16s(&name.encode_utf16().collect::<Vec<_>>()));
    rec(0x0031, &p)
}

/// Sets the family and character-set bytes of a Font record built by `font` (after the 4-byte
/// record header, 10 bytes of fields and the underline byte).
fn with_family_charset(mut font_record: Vec<u8>, family: u8, charset: u8) -> Vec<u8> {
    font_record[15] = family;
    font_record[16] = charset;
    font_record
}

/// The CellXF fields an XF record carries after ifnt, ifmt and the protection flags.
#[derive(Clone, Copy, Default)]
struct XfData {
    alc: u8,
    wrap: bool,
    alc_v: u8,
    trot: u8,
    indent: u8,
    shrink: bool,
    /// dgLeft, dgRight, dgTop, dgBottom, dgDiag.
    dg: [u32; 5],
    /// icvLeft, icvRight, icvTop, icvBottom, icvDiag.
    icv: [u32; 5],
    diag: u32,
    fls: u32,
    fore: u16,
    back: u16,
}

fn xf(ifnt: u16, ifmt: u16, flags: u16, d: XfData) -> Vec<u8> {
    let mut p = u16s(&[ifnt, ifmt, flags]);
    p.push(d.alc | (u8::from(d.wrap) << 3) | (d.alc_v << 4));
    p.push(d.trot);
    p.push(d.indent | (u8::from(d.shrink) << 4));
    p.push(0xFC); // fAtr* bits: no bearing on the values read
    let b1 = d.dg[0] | (d.dg[1] << 4) | (d.dg[2] << 8) | (d.dg[3] << 12) | (d.icv[0] << 16) | (d.icv[1] << 23) | (d.diag << 30);
    let b2 = d.icv[2] | (d.icv[3] << 7) | (d.icv[4] << 14) | (d.dg[4] << 21) | (d.fls << 26);
    p.extend_from_slice(&b1.to_le_bytes());
    p.extend_from_slice(&b2.to_le_bytes());
    p.extend_from_slice(&(d.fore | (d.back << 7)).to_le_bytes());
    rec(0x00E0, &p)
}

/// What Excel writes for unformatted cells: bottom aligned, automatic fill colours.
fn plain() -> XfData {
    XfData { alc_v: 2, fore: 0x40, back: 0x41, ..XfData::default() }
}

/// Locked, a cell XF inheriting from style XF 0.
const CELL: u16 = 0x0001;
/// Locked, a style XF with no parent.
const STYLE: u16 = 0xFFF5;

fn number(r: u16, c: u16, ixfe: u16, n: f64) -> Vec<u8> {
    let mut p = u16s(&[r, c, ixfe]);
    p.extend_from_slice(&n.to_le_bytes());
    rec(0x0203, &p)
}

fn blank(r: u16, c: u16, ixfe: u16) -> Vec<u8> {
    rec(0x0201, &u16s(&[r, c, ixfe]))
}

/// A one-sheet workbook: `formatting` records in the globals, `cells` in the sheet.
fn workbook(formatting: &[Vec<u8>], cells: &[Vec<u8>]) -> Vec<u8> {
    let boundsheet = |pos: u32| {
        let mut p = pos.to_le_bytes().to_vec();
        p.extend_from_slice(&[0, 0, 6, 0]);
        p.extend_from_slice(b"Sheet1");
        rec(0x0085, &p)
    };
    let globals = |pos: u32| [vec![bof(0x0005)], formatting.to_vec(), vec![boundsheet(pos), eof()]].concat().concat();
    let sheet = [vec![bof(0x0010)], cells.to_vec(), vec![eof()]].concat().concat();
    let len = globals(0).len() as u32;
    [globals(len), sheet].concat()
}

fn style_at<'a>(wb: &'a Workbook, a1: &str) -> &'a Style {
    wb.styles.get(wb.sheets[0].style_id(CellRef::parse(a1).unwrap()))
}

/// Four default fonts (Arial 10 and its bold/italic variants), as Excel writes them.
fn default_fonts() -> Vec<Vec<u8>> {
    vec![
        font(200, 0, 0x7FFF, 400, 0, 0, "Arial"),
        font(200, 0, 0x7FFF, 700, 0, 0, "Arial"),
        font(200, 2, 0x7FFF, 400, 0, 0, "Arial"),
        font(200, 2, 0x7FFF, 700, 0, 0, "Arial"),
    ]
}

/// Fonts, a palette, style XFs, cell XFs and named styles covering every decoded field.
fn formatted() -> Vec<u8> {
    let mut f = default_fonts();
    // ifnt 5 (index 4 is never written): 14 pt italic, struck, bold, double underline,
    // superscript, red (colour 10).
    f.push(font(280, 0x000A, 10, 700, 1, 2, "Courier New"));
    // ifnt 6: single accounting underline, subscript, a custom palette colour.
    f.push(font(220, 0, 9, 400, 2, 0x21, "Times New Roman"));
    // Palette: the first two of the 56 custom entries (colour indexes 8 and 9) replaced.
    let mut pal = u16s(&[56]);
    for (i, c) in INDEXED.iter().enumerate().skip(8) {
        let c = match i {
            8 => 0x123456,
            9 => 0xABCDEF,
            _ => *c,
        };
        pal.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 0]);
    }
    f.push(rec(0x0092, &pal));
    f.push(rec(0x041E, &[&164u16.to_le_bytes()[..], &[4u8, 0, 0][..], &b"0.0%"[..]].concat()));
    // XF 0: the Normal style.
    f.push(xf(0, 0, STYLE, plain()));
    // XF 1: everything at once.
    f.push(xf(
        5,
        164,
        0x0002, // hidden, not locked
        XfData {
            alc: 2,
            wrap: true,
            alc_v: 1,
            trot: 135,
            indent: 3,
            shrink: true,
            dg: [1, 0, 7, 2, 3],
            icv: [9, 0, 8, 0x40, 10],
            diag: 3,
            fls: 1,
            fore: 8,
            back: 0x41,
        },
    ));
    // XF 2: a 12.5% gray pattern on yellow, font index 4 (invalid: the default font).
    f.push(xf(4, 0, CELL, XfData { alc: 7, alc_v: 4, trot: 255, fls: 0x11, fore: 10, back: 13, ..plain() }));
    // XF 3: a font index past the end, right aligned, diagonal-up only.
    f.push(xf(200, 0, CELL, XfData { alc: 3, alc_v: 0, trot: 90, dg: [0, 13, 0, 0, 5], icv: [0, 12, 0, 0, 16], diag: 2, ..plain() }));
    // XF 4: an ordinary cell: the default style.
    f.push(xf(0, 0, CELL, plain()));
    // XF 5: font ifnt 6.
    f.push(xf(6, 0, CELL, plain()));
    // Styles: built-in Normal, a user style on XF 1 and a duplicate name (ignored), a built-in
    // style (Comma) left to GridCraft's own definition.
    f.push(rec(0x0293, &[&u16s(&[0x8000])[..], &[0u8, 0xFF][..]].concat()));
    f.push(rec(0x0293, &[&u16s(&[1, 5])[..], &[0u8][..], &b"Fancy"[..]].concat()));
    f.push(rec(0x0293, &[&u16s(&[2, 5])[..], &[0u8][..], &b"FANCY"[..]].concat()));
    f.push(rec(0x0293, &[&u16s(&[0x8002])[..], &[3u8, 0xFF][..]].concat()));
    workbook(
        &f,
        &[
            number(0, 0, 1, 0.5),
            blank(0, 1, 1),
            number(1, 0, 2, 2.0),
            number(2, 0, 3, 3.0),
            number(3, 0, 4, 4.0),
            number(4, 0, 999, 5.0),
            number(5, 0, 5, 6.0),
            blank(6, 0, 4),
        ],
    )
}

#[test]
fn reads_fonts_fills_borders_alignment_and_protection() {
    let (wb, report) = read_xls(&compound("Workbook", &formatted())).unwrap();
    assert!(report.warnings[0].contains("cell formatting"), "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w.contains("shorter than expected")), "{:?}", report.warnings);
    reads_primary_style(&wb);
    reads_fallback_styles(&wb);
    reads_cell_style_and_xlsx_roundtrip(&wb);
}

fn reads_primary_style(wb: &Workbook) {
    // The Normal style's font is the workbook default.
    let normal = wb.styles.get(StyleId::DEFAULT);
    assert_eq!((normal.font.name.as_str(), normal.font.size, normal.font.bold), ("Arial", 10.0, false));
    assert_eq!(normal.font.color, Color::Auto);

    let s = style_at(wb, "A1");
    let f = &s.font;
    assert_eq!((f.name.as_str(), f.size), ("Courier New", 14.0));
    assert!(f.bold && f.italic && f.strike);
    assert_eq!((f.underline, f.vert, f.color), (Underline::Double, VertAlign::Superscript, Color::Rgb(0xFF0000)));
    assert_eq!(s.num_fmt.as_str(), "0.0%");
    assert_eq!(s.protection, Protection { locked: false, hidden: true });
    assert_eq!((s.align.h, s.align.v, s.align.wrap, s.align.shrink), (HAlign::Center, VAlign::Center, true, true));
    assert_eq!((s.align.rotation, s.align.indent), (-45, 3));
    // Colour index 8 and 9 come from the Palette record.
    assert_eq!(s.fill, Fill::solid(Color::Rgb(0x123456)));
    assert_eq!(s.borders.left, BorderLine { style: BorderStyle::Thin, color: Color::Rgb(0xABCDEF) });
    assert_eq!(s.borders.right, BorderLine::default());
    assert_eq!(s.borders.top, BorderLine { style: BorderStyle::Hair, color: Color::Rgb(0x123456) });
    assert_eq!(s.borders.bottom, BorderLine { style: BorderStyle::Medium, color: Color::Auto });
    let dashed = BorderLine { style: BorderStyle::Dashed, color: Color::Rgb(0xFF0000) };
    assert_eq!((s.borders.diag_down, s.borders.diag_up), (dashed, dashed));
    // A formatted blank cell keeps its formatting.
    assert_eq!(wb.sheets[0].style_id(CellRef::parse("B1").unwrap()), wb.sheets[0].style_id(CellRef::parse("A1").unwrap()));
    assert_eq!(wb.sheets[0].value(CellRef::parse("B1").unwrap()), Value::Empty);
}

fn reads_fallback_styles(wb: &Workbook) {
    let normal = wb.styles.get(StyleId::DEFAULT);
    let s = style_at(wb, "A2");
    assert_eq!(s.font, normal.font, "font index 4 falls back to the default font");
    assert_eq!(s.fill, Fill { pattern: PatternType::Gray125, fg: Color::Rgb(0xFF0000), bg: Color::Rgb(0xFFFF00) });
    assert_eq!((s.align.h, s.align.v, s.align.rotation), (HAlign::Distributed, VAlign::Distributed, 255));

    let s = style_at(wb, "A3");
    assert_eq!(s.font, normal.font, "a font index past the end falls back to the default font");
    assert_eq!((s.align.h, s.align.v, s.align.rotation), (HAlign::Right, VAlign::Top, 90));
    assert_eq!(s.borders.right, BorderLine { style: BorderStyle::SlantDashDot, color: Color::Rgb(0x0000FF) });
    assert_eq!(s.borders.diag_up, BorderLine { style: BorderStyle::Thick, color: Color::Rgb(0x800000) });
    assert_eq!(s.borders.diag_down, BorderLine::default());

    // An ordinary cell XF is the default style; so is an XF index past the end.
    assert_eq!(wb.sheets[0].style_id(CellRef::parse("A4").unwrap()), StyleId::DEFAULT);
    assert_eq!(wb.sheets[0].style_id(CellRef::parse("A5").unwrap()), StyleId::DEFAULT);
    // A blank cell in the default style isn't stored.
    assert!(wb.sheets[0].cell(CellRef::parse("A7").unwrap()).is_none());
}

fn reads_cell_style_and_xlsx_roundtrip(wb: &Workbook) {
    let s = style_at(wb, "A6");
    assert_eq!((s.font.name.as_str(), s.font.size, s.font.color), ("Times New Roman", 11.0, Color::Rgb(0xABCDEF)));
    assert_eq!((s.font.underline, s.font.vert), (Underline::SingleAccounting, VertAlign::Subscript));

    // User-defined styles are kept (first of a name wins); built-in ones are GridCraft's own.
    assert_eq!(wb.cell_styles.len(), 1, "{:?}", wb.cell_styles.iter().map(|(n, _)| n).collect::<Vec<_>>());
    assert_eq!(wb.cell_styles[0].0, "Fancy");
    assert_eq!(&wb.cell_styles[0].1, style_at(wb, "A1"));

    // The formatting survives a save as .xlsx.
    let (back, _) = read_xlsx(&write_xlsx(wb).unwrap()).unwrap();
    for a1 in ["A1", "A2", "A3", "A6"] {
        assert_eq!(style_at(&back, a1), style_at(wb, a1), "{a1}");
    }
    assert_eq!(back.styles.get(StyleId::DEFAULT).font.name, "Arial");
    assert_eq!(back.cell_styles.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Fancy"]);
}

#[test]
fn font_family_and_charset_are_kept() {
    let mut f = default_fonts();
    // ifnt 5: family 3 (modern) and character set 0xEE (Eastern European).
    f.push(with_family_charset(font(200, 0, 0x7FFF, 400, 0, 0, "Courier New"), 3, 0xEE));
    f.push(xf(0, 0, STYLE, plain()));
    f.push(xf(5, 0, CELL, plain()));
    let (wb, _) = read_xls(&compound("Workbook", &workbook(&f, &[number(0, 0, 1, 1.0)]))).unwrap();
    let s = style_at(&wb, "A1");
    assert_eq!((s.font.name.as_str(), s.font.family, s.font.charset), ("Courier New", Some(3), Some(0xEE)));
}

#[test]
fn without_a_palette_the_default_colours_apply() {
    let mut f = default_fonts();
    f.push(xf(0, 0, STYLE, plain()));
    f.push(xf(0, 0, CELL, XfData { fls: 1, fore: 8, ..plain() }));
    f.push(xf(0, 0, CELL, XfData { fls: 1, fore: 63, ..plain() }));
    // Out-of-range colour indexes are automatic.
    f.push(xf(0, 0, CELL, XfData { fls: 1, fore: 0x7F, dg: [1, 0, 0, 0, 0], icv: [0x50, 0, 0, 0, 0], ..plain() }));
    let (wb, _) = read_xls(&compound("Workbook", &workbook(&f, &[number(0, 0, 1, 1.0), number(1, 0, 2, 1.0), number(2, 0, 3, 1.0)]))).unwrap();
    assert_eq!(style_at(&wb, "A1").fill, Fill::solid(Color::Rgb(0x000000)));
    assert_eq!(style_at(&wb, "A2").fill, Fill::solid(Color::Rgb(0x333333)));
    let s = style_at(&wb, "A3");
    assert_eq!(s.fill, Fill::solid(Color::Auto));
    assert_eq!(s.borders.left, BorderLine { style: BorderStyle::Thin, color: Color::Auto });
}

#[test]
fn short_formatting_records_keep_what_is_there() {
    let mut f = default_fonts();
    // A truncated font (no name): the default font keeps the numbering of the next one.
    f.push(rec(0x0031, &u16s(&[240, 0])));
    f.push(font(300, 0, 0x7FFF, 700, 0, 0, "Georgia"));
    // A palette claiming more entries than it holds.
    f.push(rec(0x0092, &[&u16s(&[56])[..], &[1u8, 2, 3, 0][..]].concat()));
    f.push(xf(0, 0, STYLE, plain()));
    // XF 1: only ifnt and ifmt and flags: number format kept, no alignment.
    f.push(rec(0x00E0, &u16s(&[6, 10, 0x0001])));
    // XF 2: two bytes.
    f.push(rec(0x00E0, &u16s(&[6])));
    // XF 3: palette colour 8 (overridden) as a fill.
    f.push(xf(0, 0, CELL, XfData { fls: 1, fore: 8, ..plain() }));
    // Style records cut short, and a style naming a missing XF.
    f.push(rec(0x0293, &u16s(&[0x8000])));
    f.push(rec(0x0293, &[&u16s(&[1, 40])[..], &[0u8][..], &b"Cut"[..]].concat()));
    f.push(rec(0x0293, &[&u16s(&[900, 4])[..], &[0u8][..], &b"Gone"[..]].concat()));
    let book = workbook(&f, &[number(0, 0, 1, 1.0), number(1, 0, 2, 1.0), number(2, 0, 3, 1.0)]);
    let (wb, report) = read_xls(&compound("Workbook", &book)).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("shorter than expected")), "{:?}", report.warnings);
    let s = style_at(&wb, "A1");
    assert_eq!((s.font.name.as_str(), s.font.bold), ("Georgia", true));
    assert_eq!(s.num_fmt.as_str(), "0.00%");
    assert_eq!((s.align.v, s.protection.locked), (VAlign::Bottom, true));
    assert_eq!(wb.sheets[0].style_id(CellRef::parse("A2").unwrap()), StyleId::DEFAULT);
    assert_eq!(style_at(&wb, "A3").fill, Fill::solid(Color::Rgb(0x010203)));
    assert!(wb.cell_styles.is_empty());
}

#[test]
fn hostile_formatting_records_never_panic() {
    let stream = formatted();
    for at in 0..stream.len() {
        for x in [0x00u8, 0xFF, 0x80] {
            let mut s = stream.clone();
            s[at] = x;
            let _ = read_xls(&compound("Workbook", &s));
        }
    }
    for cut in 0..stream.len() {
        let _ = read_xls(&compound("Workbook", &stream[..cut]));
    }
}
