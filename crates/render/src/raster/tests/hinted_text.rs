//! Small text is fitted to the pixel grid at the size it is drawn (vendored hayro patch 34 and
//! hayro-interpret patch 111, #730). Glyph outlines used to be filled as they are at 1000 units
//! per em, so a baseline or the top of a stem between two pixel rows came out as two grey rows:
//! 10 pt text at 96 dpi looked soft, its thin strokes grey and broken, and each line differed from
//! the next with where its baseline fell.
//!
//! Each font program the renderer reads (a standard font's stand-in, an embedded CFF, TrueType
//! and Type 1 program) draws "HHH" at 10 pt, 96 dpi, with its baseline a quarter, a half and three
//! quarters of a pixel off the grid. Down the middle of an `H`'s stem the first and last inked
//! rows are then whole: unhinted, the baseline's row was a quarter, half or three quarters inked.

use super::*;

/// 10 pt at 96 dpi: 13.33 pixels per em.
const SCALE: f32 = 96.0 / 72.0;
/// The page is 40 pt tall.
const PAGE_HEIGHT: f32 = 40.0;

/// Where a font comes from in [`pdf`].
enum Program<'a> {
    /// Not embedded: the renderer's stand-in for a standard font.
    Standard(&'a str),
    /// Embedded under `/FontFile` (Type 1), `/FontFile2` (TrueType) or `/FontFile3` (CFF).
    Embedded { subtype: &'a str, file_key: &'a str, file_subtype: Option<&'a str>, data: &'a [u8] },
}

/// A page showing "HHH" at 10 pt in `program`, its baseline `baseline` points from the bottom,
/// with `text` operators in between (a render mode, a matrix) when given.
fn pdf(program: &Program<'_>, baseline: f32, matrix: &str, operators: &str) -> Vec<u8> {
    let content = format!("BT /F1 10 Tf {operators} {matrix} 5 {baseline} Tm (HHH) Tj ET");
    let (font, file) = match program {
        Program::Standard(name) => (format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} >>"), None),
        Program::Embedded { subtype, file_key, file_subtype, data } => {
            let font =
                format!("<< /Type /Font /Subtype /{subtype} /BaseFont /Test /FirstChar 72 /LastChar 72 /Widths [700] /FontDescriptor 6 0 R >>");
            let file_subtype = file_subtype.map(|s| format!(" /Subtype /{s}")).unwrap_or_default();
            let descriptor = format!(
                "6 0 obj << /Type /FontDescriptor /FontName /Test /Flags 34 /FontBBox [0 -250 1000 950] /ItalicAngle 0 /Ascent 900 /Descent -250 /CapHeight 700 /StemV 80 /{file_key} 7 0 R >> endobj\n7 0 obj << /Length {}{file_subtype} >> stream\n",
                data.len()
            );
            (font, Some((descriptor, *data)))
        }
    };
    let mut out = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 60 {PAGE_HEIGHT}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj {font} endobj
",
        content.len()
    )
    .into_bytes();
    if let Some((descriptor, data)) = file {
        out.extend_from_slice(descriptor.as_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream endobj\n");
    }
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

/// `pdf`'s page as PdfCraft renders it at [`SCALE`].
fn render(pdf: Vec<u8>) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: SCALE, tag: 0, preview: false });
    assert!(out.error.is_none(), "{:?}", out.error);
    out
}

/// `pdf`'s page as hayro renders it at [`SCALE`], with or without hinting.
fn render_hayro(pdf: Vec<u8>, hint_text: bool) -> Vec<u8> {
    let parsed = Pdf::new(Arc::new(pdf)).expect("the test PDF parses");
    let settings = RenderConfig::default().settings();
    let rs = RenderSettings { x_scale: SCALE, y_scale: SCALE, bg_color: WHITE, hint_text, ..Default::default() };
    hayro::render(&parsed.pages()[0], &RenderCache::new(), &settings, &rs).data_as_u8_slice().to_vec()
}

/// How much of the pixel at `(x, y)` is inked, from 0 to 1, on white.
fn ink(page: &RenderedPage, x: u32, y: u32) -> f32 {
    let at = ((y * page.width + x) * 4) as usize;
    1.0 - f32::from(page.rgba[at]) / 255.0
}

/// The ink of the first and last inked rows down the most inked column (the middle of a stem).
fn stem_ends(page: &RenderedPage) -> (f32, f32) {
    let column = (0..page.width)
        .max_by(|a, b| {
            let sum = |x| (0..page.height).map(|y| ink(page, x, y)).sum::<f32>();
            sum(*a).total_cmp(&sum(*b))
        })
        .expect("the page has columns");
    let inked: Vec<u32> = (0..page.height).filter(|&y| ink(page, column, y) > 0.05).collect();
    let (first, last) = (inked.first().copied().expect("the text draws ink"), inked.last().copied().expect("the text draws ink"));
    (ink(page, column, first), ink(page, column, last))
}

/// Draws "HHH" in `program` with its baseline off the pixel grid by a quarter, a half and three
/// quarters of a pixel, and checks the stem starts and ends on whole rows.
fn assert_stems_end_on_whole_rows(name: &str, program: &Program<'_>) {
    for offset in [0.25, 0.5, 0.75] {
        // A baseline `offset` pixels below a pixel boundary, counted from the top.
        let rows_from_top = 25.0 + offset;
        let baseline = PAGE_HEIGHT - rows_from_top / SCALE;
        let page = render(pdf(program, baseline, "1 0 0 1", ""));
        let (top, bottom) = stem_ends(&page);
        assert!(
            top >= 0.9 && bottom >= 0.9,
            "{name}, baseline {offset} px off the grid: the stem's first and last rows are {top:.2} and {bottom:.2} inked"
        );
    }
}

#[test]
fn a_standard_fonts_stand_in_ends_its_stems_on_whole_rows() {
    assert_stems_end_on_whole_rows("Times-Roman", &Program::Standard("Times-Roman"));
    assert_stems_end_on_whole_rows("Helvetica", &Program::Standard("Helvetica"));
}

#[test]
fn an_embedded_cff_program_ends_its_stems_on_whole_rows() {
    let cff = include_bytes!("../../../../../vendor/hayro-interpret/assets/FoxitSerif.pfb");
    let program = Program::Embedded { subtype: "Type1", file_key: "FontFile3", file_subtype: Some("Type1C"), data: cff };
    assert_stems_end_on_whole_rows("embedded CFF", &program);
}

#[test]
fn an_embedded_truetype_program_ends_its_stems_on_whole_rows() {
    let ttf = include_bytes!("../../../../../assets/fonts/Inter-Regular.ttf");
    let program = Program::Embedded { subtype: "TrueType", file_key: "FontFile2", file_subtype: None, data: ttf };
    assert_stems_end_on_whole_rows("embedded TrueType", &program);
}

#[test]
fn an_embedded_type1_program_ends_its_stems_on_whole_rows() {
    let program = type1_program();
    let program = Program::Embedded { subtype: "Type1", file_key: "FontFile", file_subtype: None, data: &program };
    assert_stems_end_on_whole_rows("embedded Type 1", &program);
}

/// Text turned a quarter draws as it did: its glyphs' vertical axis lies along the line, and the
/// hinters fit outlines only vertically.
#[test]
fn turned_text_draws_as_before() {
    let ttf = include_bytes!("../../../../../assets/fonts/Inter-Regular.ttf");
    let program = Program::Embedded { subtype: "TrueType", file_key: "FontFile2", file_subtype: None, data: ttf };
    let page = pdf(&program, 20.3, "0 1 -1 0", "");
    let (plain, hinted) = (render_hayro(page.clone(), false), render_hayro(page, true));
    assert!(plain.as_chunks::<4>().0.iter().any(|pixel| pixel[0] < 128), "the text draws ink");
    assert!(plain == hinted, "turned text must draw exactly as unhinted");
}

/// A vertical-writing font's glyphs draw as they did. Their column advances down the page, so
/// putting each glyph's origin on a whole pixel moved it along the column: the gaps between
/// characters alternated by a pixel.
#[test]
fn vertical_writing_draws_as_before() {
    use skrifa::MetadataProvider as _;
    let ttf = include_bytes!("../../../../../assets/fonts/Inter-Regular.ttf");
    let h = skrifa::FontRef::new(ttf).expect("Inter reads").charmap().map('H').expect("Inter has H").to_u32();
    let content = format!("BT /F1 10 Tf 1 0 0 1 30 55.3 Tm <{h:04X}{h:04X}{h:04X}{h:04X}> Tj ET");
    let mut page = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 60 60] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type0 /BaseFont /Test /Encoding /Identity-V /DescendantFonts [6 0 R] >> endobj
6 0 obj << /Type /Font /Subtype /CIDFontType2 /BaseFont /Test /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>
  /FontDescriptor 7 0 R /CIDToGIDMap /Identity /DW 1000 >> endobj
7 0 obj << /Type /FontDescriptor /FontName /Test /Flags 32 /FontBBox [0 -250 1000 950] /ItalicAngle 0 /Ascent 900 /Descent -250 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >> endobj
8 0 obj << /Length {} >> stream
",
        content.len(),
        ttf.len()
    )
    .into_bytes();
    page.extend_from_slice(ttf);
    page.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");
    let (plain, hinted) = (render_hayro(page.clone(), false), render_hayro(page, true));
    assert!(plain.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 128).count() > 40, "the column draws its letters");
    assert!(plain == hinted, "vertical text must draw exactly as unhinted");
}

/// Text filled then stroked (render mode 2, a faux bold) is stroked along the edges its fill was
/// fitted to: down the middle of a stem, a one-pixel stroke half inks the row above the stem's
/// top and the row below its foot. Stroked along the unhinted outline, which lies between rows
/// where the fill doesn't, it inked those rows unevenly, a quarter here and three quarters there.
#[test]
fn text_filled_then_stroked_is_stroked_along_its_fitted_edges() {
    let ttf = include_bytes!("../../../../../assets/fonts/Inter-Regular.ttf");
    let program = Program::Embedded { subtype: "TrueType", file_key: "FontFile2", file_subtype: None, data: ttf };
    for offset in [0.25, 0.5, 0.75] {
        let baseline = PAGE_HEIGHT - (25.0 + offset) / SCALE;
        // 0.75 pt is one pixel at 96 dpi.
        let page = render(pdf(&program, baseline, "1 0 0 1", "2 Tr 0.75 w"));
        let (top, bottom) = stem_ends(&page);
        assert!(
            (0.35..=0.65).contains(&top) && (0.35..=0.65).contains(&bottom),
            "baseline {offset} px off the grid: the stroke inks the rows outside the stem {top:.2} and {bottom:.2}"
        );
    }
}

/// An accented letter a standard font's stand-in lacks, which the renderer builds from the base
/// letter and a mark, is fitted like the base letter drawn beside it: Helvetica's `ț` is the
/// stand-in's `t` with its comma under it, and its `t` must be that `t`'s pixels. Built from
/// unhinted parts, it sat a row higher than the hinted `t` and its comma touched the letter.
#[test]
fn an_accented_letter_built_from_two_glyphs_matches_its_base_letter() {
    let content = "BT /F1 40 Tf 10 20 Td <01> Tj ET\nBT /F1 40 Tf 50 20 Td <02> Tj ET\n";
    let pdf = format!(
        "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica
  /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [1 /t /uni021B] >> >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    );
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    // The baseline is 60 rows from the top; the letters are 40 columns apart.
    let (rgba, width) = (&page.rgba, page.width);
    let cell = |left: u32| -> Vec<u8> { (0..=60).flat_map(|y| (left..left + 30).map(move |x| rgba[((y * width + x) * 4) as usize])).collect() };
    let (t, t_comma) = (cell(5), cell(45));
    assert!(t.iter().any(|&v| v < 128), "the t draws ink");
    assert!(t == t_comma, "the t of ț must be the t's pixels");
}

/// Hinting off renders as before the patch: hayro's default leaves glyph outlines as they are.
#[test]
fn hayro_hints_only_when_asked() {
    let page = pdf(&Program::Standard("Times-Roman"), 20.3, "1 0 0 1", "");
    assert!(render_hayro(page.clone(), false) != render_hayro(page.clone(), true), "hinting moves the text's edges");
    assert!(!RenderSettings::default().hint_text, "hayro leaves glyphs unhinted unless asked");
}

/// A Type 1 program with one glyph, `H`: two 80-unit stems 700 units tall and a bar between
/// them, its blue zones at the baseline and at 700. Written as the Type 1 format describes
/// (Adobe Type 1 Font Format, chapters 6 and 7): charstrings encrypted with key 4330 and four
/// leading bytes, the private part with eexec's key 55665.
fn type1_program() -> Vec<u8> {
    fn number(out: &mut Vec<u8>, v: i32) {
        match v {
            -107..=107 => out.push((v + 139) as u8),
            108..=1131 => {
                let w = v - 108;
                out.extend([(w / 256 + 247) as u8, (w % 256) as u8]);
            }
            -1131..=-108 => {
                let w = -v - 108;
                out.extend([(w / 256 + 251) as u8, (w % 256) as u8]);
            }
            _ => {
                out.push(255);
                out.extend(v.to_be_bytes());
            }
        }
    }
    fn encrypt(plain: &[u8], mut r: u16) -> Vec<u8> {
        plain
            .iter()
            .map(|&p| {
                let c = p ^ (r >> 8) as u8;
                r = (u16::from(c).wrapping_add(r)).wrapping_mul(52845).wrapping_add(22719);
                c
            })
            .collect()
    }
    const RMOVETO: u8 = 21;
    const RLINETO: u8 = 5;
    const CLOSEPATH: u8 = 9;
    const HSBW: u8 = 13;
    const ENDCHAR: u8 = 14;
    let charstring = |ops: &[(Vec<i32>, u8)]| {
        let mut plain = vec![0; 4];
        for (args, op) in ops {
            for &a in args {
                number(&mut plain, a);
            }
            plain.push(*op);
        }
        encrypt(&plain, 4330)
    };
    // A rectangle `w` by `h` whose lower left corner is `(dx, dy)` from the current point. It
    // ends where it began: `closepath` leaves the current point at the last point drawn.
    let rect = |dx: i32, dy: i32, w: i32, h: i32| {
        [(vec![dx, dy], RMOVETO), (vec![w, 0], RLINETO), (vec![0, h], RLINETO), (vec![-w, 0], RLINETO), (vec![0, -h], RLINETO), (vec![], CLOSEPATH)]
    };
    let notdef = charstring(&[(vec![0, 500], HSBW), (vec![], ENDCHAR)]);
    let mut h_ops = vec![(vec![0, 700], HSBW)];
    h_ops.extend(rect(100, 0, 80, 700));
    h_ops.extend(rect(420, 0, 80, 700));
    h_ops.extend(rect(-340, 320, 340, 60));
    h_ops.push((vec![], ENDCHAR));
    let h = charstring(&h_ops);

    let mut private = b"dup /Private 8 dict dup begin\n/RD{string currentfile exch readstring pop}executeonly def\n/ND{noaccess def}executeonly def\n/NP{noaccess put}executeonly def\n/lenIV 4 def\n/BlueValues [-12 0 700 712] def\n/StdHW [60] def\n/StdVW [80] def\n/password 5839 def\n/MinFeature {16 16} def\n2 index /CharStrings 2 dict dup begin\n".to_vec();
    for (name, cs) in [("/.notdef", &notdef), ("/H", &h)] {
        private.extend(format!("{name} {} RD ", cs.len()).as_bytes());
        private.extend_from_slice(cs);
        private.extend(b" ND\n");
    }
    private.extend(b"end\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n");
    let mut eexec = vec![0; 4];
    eexec.extend(private);

    let mut font = b"%!PS-AdobeFont-1.0: Test 001.000\n12 dict begin\n/FontName /Test def\n/FontType 1 def\n/PaintType 0 def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/FontBBox {0 0 700 700} readonly def\n/Encoding 256 array 0 1 255 {1 index exch /.notdef put} for dup 72 /H put readonly def\ncurrentdict end\ncurrentfile eexec\n".to_vec();
    font.extend(encrypt(&eexec, 55665));
    font
}
