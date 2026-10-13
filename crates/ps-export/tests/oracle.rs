//! The Ghostscript oracle: export → gs raster → compare against PdfCraft's own raster.
//!
//! Layer A's correctness claim is measured here, not asserted: for every fixture class the
//! exported PostScript is rasterized by Ghostscript (`png16m` at 50 dpi, the clean-room
//! external-process oracle the repo already uses for qpdf/CUPS-style gates) and the source
//! PDF by `pdfcraft_render` at the same effective dpi, and the mean per-channel delta of
//! the two rasters must sit under [`TOLERANCE`]. gs runs as an observed oracle process,
//! never linked; where it is not installed the raster fixtures skip with a message (the
//! CUPS-test precedent). Transparency is asserted only as "completes + note" — flattening
//! is Layer C, so a transparent page never enters the raster comparison.
//!
//! Fixture pages are 288×360 pt, a whole multiple of the 1.44 pt pixel at 50 dpi, so both
//! rasters are exactly 200×250 px and the pixel grids coincide without rounding luck.
//!
//! The W2 merge gates (CFF promotion, CID composites) prove the composite arms the
//! same way, with one addition: on this gs build txtwrite maps composite text to its
//! raw codes as Unicode, never to glyph names, so those gates assert structure, ink
//! and raster parity, leaving extraction to the name-carrying base-font arms (the
//! tests' doc comments carry the probes). Their craft-fonts fixtures are read at test
//! runtime from `CRAFT_FONTS_DIR` when a checkout exists — the raster-crate
//! precedent — so no font bytes ride in the repository.

use std::io::Cursor;
use std::process::Command;
use std::sync::Arc;

use pdfcraft_ps_export::{PsFormat, export};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

/// The render scale both sides use: 50 dpi, the oracle's `-r50`.
const SCALE: f32 = 50.0 / 72.0;
/// The canvas every fixture page occupies at [`SCALE`] (288×360 pt → 200×250 px, exact).
const PX: (u32, u32) = (200, 250);
/// Mean per-channel delta, in 0–255 byte levels, below which a fixture's two rasters count
/// as the same image. The two rasterizers (vello_cpu and Ghostscript) quantize edge
/// coverage differently (8-bit analytic coverage against 4-bit GraphicsAlphaBits), so a
/// small delta is inherent on every antialiased edge; the observed worst case across the
/// corpus is ≈ 0.9 (real-font TrueType, the densest edge coverage). The value keeps ≈ 2×
/// headroom over that worst case — enough to stay stable across gs builds, tight enough
/// that a mis-emitted operator (wrong colour, dropped draw, misplaced box) fails loudly:
/// any of those moves whole regions by tens of levels per channel.
const TOLERANCE: f64 = 2.0;

// ----------------------------------------------------------------- fixture PDFs

/// A `<< … >> stream … endstream` object body (the repo fixture style; lengths are
/// computed, never counted by hand).
fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
    let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
    obj.extend_from_slice(body);
    obj.extend_from_slice(b"\nendstream");
    obj
}

/// Assemble a PDF from object bodies (numbered from 1), computing the xref like
/// `crates/engine/src/tests.rs::fixture`.
fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A one-page 288×360 PDF whose content draws `body`, with `resources` on the page.
fn one_page(resources: &str, body: &[u8]) -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("", body),
        format!("<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << {resources} >> >>").into_bytes(),
    ])
}

/// Filled and stroked vector geometry: rectangles (both fill rules), lines, a dashed
/// stroke, a filled Bézier shape and round-join strokes — the Task 3 emission surface.
fn geometry_pdf() -> Vec<u8> {
    one_page(
        "",
        b"1 0 0 rg\n20 20 100.8 72 re f\n\
          0 1 0 rg\n158.4 14.4 115.2 86.4 re f*\n\
          0 0 1 RG\n3 w\n14.4 115.2 m 273.6 172.8 l S\n\
          0.75 g\n1 w\n[3 2] 0 d\n14.4 129.6 m 273.6 187.2 l S\n\
          [] 0 d\n0 1 1 rg\n43.2 216 m 115.2 345.6 72 216 158.4 316.8 c h f\n\
          1 0 1 rg\n187.2 230.4 72 86.4 re f\n\
          0 0 0 RG\n2 w 1 J 1 j\n187.2 216 m 259.2 331.2 l\n187.2 331.2 m 259.2 216 l S",
    )
}

/// A 100×125 baseline JPEG with a smooth two-axis colour gradient, encoded at test time
/// (contributor-generated bytes; no fixture file rides in the repo). Both rasters decode
/// the same JPEG, so only IDCT rounding can differ between decoders.
fn jpeg_gradient(width: u32, height: u32) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    let last = (width - 1, height - 1);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let r = (x * 255 / last.0) as u8;
        let g = (y * 255 / last.1) as u8;
        *px = image::Rgb([r, g, r / 2 + g / 3]);
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .expect("encoding the gradient JPEG cannot fail");
    bytes
}

/// A page drawing one DCTDecode image 1:1 at the oracle's pixel grid: 100×125 samples
/// placed at (28.8, 72) pt sized 144×180 pt — exactly 100×125 px at (20, 50) at 50 dpi,
/// so neither rasterizer interpolates.
fn image_pdf() -> Vec<u8> {
    let jpeg = jpeg_gradient(100, 125);
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [5 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("/Type /XObject /Subtype /Image /Width 100 /Height 125 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", &jpeg),
        stream_obj("", b"q 144 0 0 180 28.8 72 cm /Im1 Do Q"),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 3 0 R >> >> >>".to_vec(),
    ])
}

/// Group 4 fax bytes that decode to an all-black image — the renderer's own #3308 fixture
/// (`crates/render/src/raster.rs`), where `/BlackIs1 true` is observed to turn the same
/// bytes white in PdfCraft, MuPDF and Acrobat alike.
const ALL_BLACK_FAX_8X4: [u8; 6] = [0x26, 0xa2, 0xfe, 0x00, 0x20, 0x02];

/// A page drawing that 8×4 Group 4 fax image on the oracle's pixel grid, with or without
/// `/BlackIs1 true`. PDF and PostScript give the flag the same meaning (decoded 1s are
/// black) and both images carry the identity `/Decode`, so the passthrough must keep the
/// polarity the source itself renders — `/BlackIs1 true` inverts on both sides alike, and
/// no compensating `/Decode [1 0]` may appear in the export.
fn fax_pdf(black_is_1: bool) -> Vec<u8> {
    let parms = if black_is_1 { "/K -1 /Columns 8 /Rows 4 /BlackIs1 true" } else { "/K -1 /Columns 8 /Rows 4" };
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [5 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj(
            &format!(
                "/Type /XObject /Subtype /Image /Width 8 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << {parms} >>"
            ),
            &ALL_BLACK_FAX_8X4,
        ),
        stream_obj("", b"q 144 0 0 180 28.8 72 cm /Im1 Do Q"),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 3 0 R >> >> >>".to_vec(),
    ])
}

// The Type 1 program builder and its encryption helpers mirror the unit-test builders in
// `src/fonts.rs` (a minimal, real PFA program gs loads and self-defines); the TrueType
// builder mirrors `src/fonts.rs::ttf_program` (a 1000-upem font with two box glyphs).

/// Adobe Type 1 encryption (eexec seed 55665, charstring seed 4330).
fn t1_encrypt(seed: u32, data: &[u8]) -> Vec<u8> {
    let mut seed = seed;
    data.iter()
        .map(|&plain| {
            let cipher = plain ^ ((seed >> 8) as u8);
            seed = (u32::from(cipher).wrapping_add(seed)).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
            cipher
        })
        .collect()
}

/// A minimal, real Type 1 font program (PFA style, hex eexec): `.notdef`, `A` and `B`,
/// each a 300×400 box outline in a 1000-unit em (see `src/fonts.rs::t1_program`).
fn t1_program() -> Vec<u8> {
    const BOX_A: [u8; 20] = [139, 248, 136, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14];
    const BOX_B: [u8; 20] = [139, 249, 80, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14];
    const NOTDEF: [u8; 5] = [139, 248, 136, 13, 14];
    let mut private: Vec<u8> = Vec::new();
    private.extend_from_slice(
        b"dup /Private 8 dict dup begin\n\
          /RD {string currentfile exch readstring pop} executeonly def\n\
          /ND {noaccess def} executeonly def\n\
          /NP {noaccess put} executeonly def\n\
          /lenIV 0 def\n\
          2 index /CharStrings 3 dict dup begin\n",
    );
    for (name, ops) in [(".notdef", NOTDEF.as_slice()), ("A", BOX_A.as_slice()), ("B", BOX_B.as_slice())] {
        let cipher = t1_encrypt(4330, ops);
        private.extend_from_slice(format!("/{name} {} RD ", cipher.len()).as_bytes());
        private.extend_from_slice(&cipher);
        private.extend_from_slice(b"ND\n");
    }
    private.extend_from_slice(
        b"end\nend\nreadonly put\nnoaccess put\n\
          dup /FontName get exch definefont pop\n\
          mark currentfile closefile\n",
    );
    let mut plain = vec![0x5a, 0xa5, 0x37, 0xc3];
    plain.extend_from_slice(&private);
    let cipher = t1_encrypt(55665, &plain);
    #[rustfmt::skip]
    let cleartext: &str = "%!PS-AdobeFont-1.0: PdfCraftTest 001.001\n\
         11 dict begin\n\
         /FontName /PdfCraftTest def\n\
         /PaintType 0 def\n\
         /FontType 1 def\n\
         /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n\
         /Encoding StandardEncoding def\n\
         /FontBBox [0 -200 700 800] readonly def\n\
         currentdict end\n\
         currentfile eexec\n";
    let mut out = cleartext.as_bytes().to_vec();
    out.extend_from_slice(cipher.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes());
    out.push(b'\n');
    for _ in 0..8 {
        out.extend_from_slice(&[b'0'; 64]);
        out.push(b'\n');
    }
    out.extend_from_slice(b"cleartomark\n");
    out
}

/// A minimal TrueType font: 1000 upem, three glyphs (two 300×400 box outlines), a (1,0)
/// format-6 cmap mapping A→1 and B→2, and a version-3.0 `post` table (see
/// `src/fonts.rs::ttf_program`).
fn ttf_program() -> Vec<u8> {
    fn be16(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }
    fn be32(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }
    let mut head = Vec::new();
    head.extend_from_slice(&be32(0x0001_0000));
    head.extend_from_slice(&be32(0x0001_0000));
    head.extend_from_slice(&be32(0));
    head.extend_from_slice(&be32(0x5F0F_3CF5));
    head.extend_from_slice(&be16(0x0003));
    head.extend_from_slice(&be16(1000));
    head.extend_from_slice(&[0; 8]);
    head.extend_from_slice(&[0; 8]);
    head.extend_from_slice(&be16(0));
    head.extend_from_slice(&be16(0xFF38));
    head.extend_from_slice(&be16(700));
    head.extend_from_slice(&be16(800));
    head.extend_from_slice(&be16(0));
    head.extend_from_slice(&be16(8));
    head.extend_from_slice(&be16(2));
    head.extend_from_slice(&be16(0));
    head.extend_from_slice(&be16(0));
    let mut hhea = Vec::new();
    hhea.extend_from_slice(&be32(0x0001_0000));
    for v in [800i16, -200, 0, 700, 0, 0, 700, 1, 0, 0] {
        hhea.extend_from_slice(&be16(v as u16));
    }
    hhea.extend_from_slice(&[0; 8]);
    hhea.extend_from_slice(&be16(0));
    hhea.extend_from_slice(&be16(1));
    let hmtx = [be16(600), be16(0), be16(0), be16(0)].concat();
    let mut maxp = Vec::new();
    maxp.extend_from_slice(&be32(0x0001_0000));
    maxp.extend_from_slice(&be16(3));
    maxp.extend_from_slice(&[0; 26]);
    let mut cmap = Vec::new();
    cmap.extend_from_slice(&be16(0));
    cmap.extend_from_slice(&be16(1));
    cmap.extend_from_slice(&be16(1));
    cmap.extend_from_slice(&be16(0));
    cmap.extend_from_slice(&be32(12));
    cmap.extend_from_slice(&be16(6));
    cmap.extend_from_slice(&be16(14));
    cmap.extend_from_slice(&be16(0));
    cmap.extend_from_slice(&be16(65));
    cmap.extend_from_slice(&be16(2));
    cmap.extend_from_slice(&be16(1));
    cmap.extend_from_slice(&be16(2));
    let mut post = Vec::new();
    post.extend_from_slice(&be32(0x0003_0000));
    post.extend_from_slice(&be32(0));
    post.extend_from_slice(&be16(0xFF9C));
    post.extend_from_slice(&be16(50));
    post.extend_from_slice(&be32(0));
    post.extend_from_slice(&[0; 16]);
    fn box_glyph() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(&be16(1));
        g.extend_from_slice(&be16(100));
        g.extend_from_slice(&be16(0));
        g.extend_from_slice(&be16(400));
        g.extend_from_slice(&be16(400));
        g.extend_from_slice(&be16(3));
        g.extend_from_slice(&be16(0));
        g.extend_from_slice(&[0x01; 4]);
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
    let loca = [0u16, 0, 17, 34].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>();

    let tables: [(&[u8; 4], Vec<u8>); 8] =
        [(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"post", post)];
    let mut out = Vec::new();
    out.extend_from_slice(&be32(0x0001_0000));
    out.extend_from_slice(&be16(tables.len() as u16));
    out.extend_from_slice(&be16(16 * 8));
    out.extend_from_slice(&be16(3));
    out.extend_from_slice(&be16(0));
    let mut offset = 12 + 16 * tables.len();
    let mut dir = Vec::new();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        dir.extend_from_slice(tag.as_slice());
        dir.extend_from_slice(&be32(0));
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

/// A minimal CFF (Type1C) font: `.notdef`, `A` (one curve) and `B` (a box).
/// Copied from `src/fonts.rs`'s test module (house style: each test module
/// owns its fixtures) for the CFF-promotion gate's no-checkout arm.
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

/// A text page over the Layer A/B strategies: embedded Type 1 (`show` text through the
/// prolog font), embedded TrueType (Type 42 `show` text through the emitted dictionary)
/// and a non-embedded font (vectorized substitute outlines). Each draws at 72 pt
/// so the boxes cover real area at the oracle's dpi.
fn text_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [11 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        // F1: the embedded Type 1 (descriptor 5, program 6).
        b"<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftTest /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>".to_vec(),
        // F2: non-embedded Helvetica (substitute outlines, vectorized).
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /PdfCraftTest /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 6 0 R >>".to_vec(),
        stream_obj("", &t1_program()),
        // F3: the embedded synthetic TrueType (program 7, dict 8, descriptor 9).
        stream_obj("", &ttf_program()),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /PdfCraftTT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 9 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
        stream_obj("", b"1 0 0 rg\nBT /F1 72 Tf 14.4 250 Td (AB) Tj ET\n0 0 1 rg\nBT /F2 72 Tf 170 250 Td (P) Tj ET\n0 0.5 0 rg\nBT /F3 72 Tf 14.4 100 Td (AB) Tj ET"),
        b"<< /Type /Page /Parent 2 0 R /Contents 10 0 R /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 8 0 R >> >> >>".to_vec(),
    ])
}

/// A page of real-font TrueType text (Inter Regular, the repo's OFL asset, read at test
/// time — never committed as bytes): the Type 42 arm draws it through the exported
/// dictionary, and gs's FreeType-rendered outlines must match the ones the raster
/// renderer draws from the same sfnts.
fn inter_pdf() -> Option<Vec<u8>> {
    let path = format!("{}/../../assets/fonts/Inter-Regular.ttf", env!("CARGO_MANIFEST_DIR"));
    let inter = std::fs::read(path).ok()?;
    Some(assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Inter-Regular /Encoding /WinAnsiEncoding /FontDescriptor 5 0 R >>".to_vec(),
        stream_obj("", b"BT /F1 36 Tf 14.4 280 Td (Hamburg) Tj ET\nBT /F1 36 Tf 14.4 220 Td (VOID123) Tj ET"),
        b"<< /Type /FontDescriptor /FontName /Inter-Regular /Flags 32 /FontBBox [0 -200 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>".to_vec(),
        stream_obj("", &inter),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]))
}

/// A page of shadings: one axial (type 2) and one radial (type 3), each painted through a
/// clip with `sh` — the Task 6 `shfill` emission surface.
fn shading_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("", b"q 14.4 187.2 129.6 158.4 re W n /Sh1 sh Q\nq 14.4 14.4 129.6 158.4 re W n /Sh2 sh Q"),
        b"<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << /Shading << /Sh1 5 0 R /Sh2 6 0 R >> >> >>".to_vec(),
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [14.4 187.2 144 345.6] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> /Extend [true true] >>".to_vec(),
        b"<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [79.2 93.6 0 79.2 93.6 93.6] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 1 0] /C1 [1 1 0] /N 1 >> /Extend [true true] >>".to_vec(),
    ])
}

/// A page with a `/BM /Multiply` ExtGState: transparency degrades to a note (Layer A
/// policy), so this fixture never enters the raster comparison — it must export to
/// completion with the note recorded.
fn transparency_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> >> >>".to_vec(),
        stream_obj("", b"q /GS1 gs 1 0 0 rg 20 40 160 60 re f Q\nq /GS1 gs 0 1 0 rg 20 240 160 60 re f Q"),
        b"<< /Type /ExtGState /BM /Multiply >>".to_vec(),
    ])
}

/// A page painting an *oversized* free-form mesh shading: 1500 triangles whose re-encoded
/// `/DataSource` (67500 bytes) exceeds the 65535-byte PostScript string limit, so the export
/// degrades the shading to its flat mid-tone — and that fallback must actually paint ink.
/// (The pre-fix fallback emitted `fill` after a path-consuming `clip`; gs rendered a blank
/// page.) Not raster-compared against PdfCraft's render of the source: the raster renderer
/// draws the real mesh, the export honestly paints the disclosed flat stand-in — so the
/// assertion is the oracle-side ink, not the delta.
fn degraded_shading_pdf() -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();
    for i in 0..1500u32 {
        let x = i as u8;
        for vertex in [(x, 0x00u8), (x.wrapping_add(1), 0x64), (x.wrapping_add(2), 0x00)] {
            data.push(0x00); // flag: a complete vertex record
            data.push(vertex.0);
            data.push(vertex.1);
            data.extend_from_slice(&[0xff, 0x00, 0x00]); // red
        }
    }
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("", b"q /Sh1 sh Q"),
        b"<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << /Shading << /Sh1 5 0 R >> >> >>".to_vec(),
        stream_obj(
            "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [0 255 0 100 0 1 0 1 0 1]",
            &data,
        ),
    ])
}

// --------------------------------------------------- craft-fonts (test runtime)

/// Family-name prefixes AGENTS.md §1.1 forbids: Adobe's Source families and
/// Noto CJK, which is Source Han under another name. The same list
/// `crates/fonts/build.rs` keeps; the runtime gate skips those faces for the
/// same reason the build never embeds them.
const BARRED_FAMILIES: [&str; 5] = ["Source Han", "Source Serif", "Source Sans", "Noto Sans CJK", "Noto Serif CJK"];

/// The flavor of a font program, detected from its magic bytes: a bare CFF
/// starts `01 00`, an sfnt-wrapped CFF is `OTTO`, a TrueType sfnt is
/// `0001 0000`.
#[derive(Debug)]
enum FaceFlavor {
    Cff,
    SfntCff,
    TrueType,
}

/// The flavor of `bytes` from its leading magic, `None` when unrecognized
/// (a `ttcf` collection, or not a font at all).
fn flavor_of(bytes: &[u8]) -> Option<FaceFlavor> {
    match bytes.first_chunk::<4>() {
        Some([0x01, 0x00, ..]) => Some(FaceFlavor::Cff),
        Some(b"OTTO") => Some(FaceFlavor::SfntCff),
        Some([0x00, 0x01, 0x00, 0x00]) => Some(FaceFlavor::TrueType),
        _ => None,
    }
}

/// A face from a craft-fonts checkout read at test runtime (`CRAFT_FONTS_DIR`,
/// manifest `fonts/manifest.txt`, `family | style | file | scripts` lines — the
/// build script's format). `None` (after a skip message) when the checkout or a
/// matching face is absent. The flavor is detected from the bytes' magic
/// (bare CFF `01 00 …`, `OTTO`, `0001 0000`).
///
/// A face matches when its `scripts` list shares an entry with `scripts`. The
/// reader is robust where the build script is strict — a malformed line, an
/// unreadable file or an unknown magic skips that face with a message instead
/// of failing the test (AGENTS.md §4: bad input, not a crash) — and it skips
/// the [`BARRED_FAMILIES`] faces exactly as the build input does. No font
/// bytes enter the repository: the checkout is read where it stands, like
/// `CRAFT_FONTS_DIR` at build time.
fn craft_face(scripts: &[&str]) -> Option<(Vec<u8>, FaceFlavor)> {
    let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") else {
        eprintln!("skipping the craft-fonts oracle fixtures: CRAFT_FONTS_DIR is unset");
        return None;
    };
    let dir = std::path::PathBuf::from(dir);
    let manifest = dir.join("fonts/manifest.txt");
    let text = match std::fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("skipping the craft-fonts oracle fixtures: {} cannot be read ({why})", manifest.display());
            return None;
        }
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let fields: Vec<&str> = line.split(" | ").map(str::trim).collect();
        let [family, _style, file, face_scripts, ..] = fields.as_slice() else {
            eprintln!("craft-fonts manifest: skipping malformed line {line:?}");
            continue;
        };
        if BARRED_FAMILIES.iter().any(|barred| family.starts_with(barred)) {
            eprintln!("craft-fonts manifest: skipping {family} (AGENTS.md §1.1)");
            continue;
        }
        if !face_scripts.split(',').any(|s| scripts.contains(&s.trim())) {
            continue;
        }
        let bytes = match std::fs::read(dir.join(file)) {
            Ok(bytes) => bytes,
            Err(why) => {
                eprintln!("craft-fonts manifest: {file} cannot be read ({why}); skipped");
                continue;
            }
        };
        let Some(flavor) = flavor_of(&bytes) else {
            eprintln!("craft-fonts manifest: {file} has no known font magic; skipped");
            continue;
        };
        return Some((bytes, flavor));
    }
    eprintln!("skipping the craft-fonts oracle fixtures: the manifest lists no face covering {}", scripts.join("/"));
    None
}

// ------------------------------------------------------------------ the oracle

/// Render page 0 of `pdf` with PdfCraft's own rasterizer at the oracle's dpi: an opaque
/// white-backed raster (the same pixels `page_render` returns), as RGB bytes.
fn pdfcraft_raster(pdf: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut renderer = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale: SCALE, ..Default::default() });
    assert!(page.error.is_none(), "PdfCraft raster failed: {:?}", page.error);
    // The buffer's bytes are R, G, B, A in memory order (`Pixels`); the white base the
    // renderer paints first makes A = 255 everywhere, so the RGB triplets stand alone.
    let rgba = page.rgba.as_ref();
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&px[..3]);
    }
    (page.width, page.height, rgb)
}

/// gs's base arguments. `-dGraphicsAlphaBits=4 -dTextAlphaBits=4` align gs's rasterization
/// policy with the raster renderer's: vello antialiases every edge analytically, while
/// png16m's *default* graphics alpha is 1 bit, which renders the exported paths and thin
/// strokes as unantialiased silhouettes — up to 3× heavier ink than PdfCraft's raster for
/// the same program (measured; geometry 2.72 → 0.73 mean delta with the bits set). The
/// comparison is "same picture under matched policy", not "same default knobs".
fn gs_args(output: &std::path::Path) -> Vec<String> {
    [
        "-dSAFER".to_string(),
        "-dBATCH".to_string(),
        "-dNOPAUSE".to_string(),
        "-sDEVICE=png16m".to_string(),
        "-r50".to_string(),
        "-dGraphicsAlphaBits=4".to_string(),
        "-dTextAlphaBits=4".to_string(),
        format!("-g{}x{}", PX.0, PX.1),
        format!("-sOutputFile={}", output.display()),
    ]
    .to_vec()
}

/// Rasterize the exported PostScript with Ghostscript at 50 dpi into a 200×250 RGB
/// raster. `None` (after a skip message) when gs is not installed; a failed gs run fails
/// the test with gs's own stderr — the oracle exists to catch broken programs.
fn gs_raster(ps: &[u8], tag: &str) -> Option<(u32, u32, Vec<u8>)> {
    let base = std::env::temp_dir().join(format!("pdfcraft-ps-oracle-{}-{tag}", std::process::id()));
    let ps_path = base.with_extension("ps");
    let png_path = base.with_extension("png");
    std::fs::write(&ps_path, ps).expect("writing the oracle's temp .ps cannot fail");
    let run = Command::new("gs").args(gs_args(&png_path)).arg(&ps_path).output();
    let output = match run {
        Ok(output) => output,
        Err(why) => {
            eprintln!("skipping the {tag} oracle fixture: gs could not be spawned ({why})");
            let _ = std::fs::remove_file(&ps_path);
            return None;
        }
    };
    assert!(output.status.success(), "gs rejected the exported {tag} program:\n{}", String::from_utf8_lossy(&output.stderr));
    let png = std::fs::read(&png_path).expect("gs wrote its output PNG");
    let _ = std::fs::remove_file(&ps_path);
    let _ = std::fs::remove_file(&png_path);
    Some(decode_gs_png(&png, tag))
}

/// Decode gs's output PNG: 8-bit RGB at exactly the oracle canvas size.
fn decode_gs_png(png: &[u8], tag: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(Cursor::new(png));
    let mut reader = decoder.read_info().expect("gs's PNG decodes");
    let mut buf = vec![0; reader.output_buffer_size().expect("gs output size is known")];
    let info = reader.next_frame(&mut buf).expect("gs's PNG has a first frame");
    assert_eq!((info.width, info.height), PX, "gs raster size for {tag}");
    assert!(
        info.color_type == png::ColorType::Rgb && info.bit_depth == png::BitDepth::Eight,
        "png16m writes 8-bit RGB ({} got {:?}/{:?})",
        tag,
        info.color_type,
        info.bit_depth
    );
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The mean absolute per-channel difference of two RGB rasters (0–255 byte levels).
fn mean_channel_delta(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "the rasters cover the same pixel count");
    assert_eq!(a.len() % 3, 0, "both rasters are RGB");
    // Both rasters are non-empty (PX is constant), so the divisor is never zero.
    let mut total: u64 = 0;
    for (x, y) in a.as_chunks::<3>().0.iter().zip(b.as_chunks::<3>().0) {
        for i in 0..3 {
            total = total.saturating_add(x[i].abs_diff(y[i]) as u64);
        }
    }
    total as f64 / a.len() as f64
}

/// Export `pdf` as PostScript, rasterize both sides and compare: the whole oracle loop.
/// Returns the achieved mean delta, or `None` when gs is absent (already skipped).
fn assert_matches_oracle(tag: &str, pdf: &[u8]) -> Option<f64> {
    let out = export(Arc::new(pdf.to_vec()), None, &[0], PsFormat::Ps).expect("the fixture exports");
    let gs = gs_raster(&out.bytes, tag)?;
    let mine = pdfcraft_raster(pdf);
    assert_eq!((gs.0, gs.1), (mine.0, mine.1), "the two rasters of {tag} cover the same canvas");
    let delta = mean_channel_delta(&gs.2, &mine.2);
    eprintln!("{tag}: mean per-channel delta {delta:.3}/255 (tolerance {TOLERANCE})");
    if let Ok(dir) = std::env::var("PDFCRAFT_ORACLE_DUMP") {
        dump_png(std::path::Path::new(&dir).join(format!("{tag}-gs.png")), gs.0, gs.1, &gs.2);
        dump_png(std::path::Path::new(&dir).join(format!("{tag}-mine.png")), mine.0, mine.1, &mine.2);
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{tag}.pdf")), pdf);
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{tag}.ps")), &out.bytes);
    }
    assert!(delta < TOLERANCE, "{tag}: gs and PdfCraft rasters diverge (mean per-channel delta {delta:.3} ≥ {TOLERANCE})");
    Some(delta)
}

/// Write an RGB raster as PNG (diagnostics behind `PDFCRAFT_ORACLE_DUMP`).
fn dump_png(path: std::path::PathBuf, width: u32, height: u32, rgb: &[u8]) {
    let mut file = std::fs::File::create(&path).expect("dump file");
    {
        let mut enc = png::Encoder::new(&mut file, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("dump header");
        w.write_image_data(rgb).expect("dump data");
    }
    let _ = file.sync_all();
}

/// The exported PS's extractable text via gs's txtwrite device (the ps2ascii
/// engine). `None` (after a skip message) when gs is not installed. `-q` keeps
/// gs's banner out of the capture, so the string is exactly what txtwrite
/// wrote (CRLF line endings included — assert with `contains`, not equality).
/// Task 6's CFF/CJK gates reuse this.
fn gs_text(ps: &[u8], tag: &str) -> Option<String> {
    let base = std::env::temp_dir().join(format!("pdfcraft-ps-oracle-{}-{tag}-txt", std::process::id()));
    let ps_path = base.with_extension("ps");
    std::fs::write(&ps_path, ps).expect("writing the oracle's temp .ps cannot fail");
    let run = Command::new("gs").args(["-q", "-dSAFER", "-dBATCH", "-dNOPAUSE", "-sDEVICE=txtwrite", "-sOutputFile=-"]).arg(&ps_path).output();
    let output = match run {
        Ok(output) => output,
        Err(why) => {
            eprintln!("skipping the {tag} text oracle: gs could not be spawned ({why})");
            let _ = std::fs::remove_file(&ps_path);
            return None;
        }
    };
    assert!(output.status.success(), "gs rejected the exported {tag} program for text extraction:\n{}", String::from_utf8_lossy(&output.stderr));
    let _ = std::fs::remove_file(&ps_path);
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Geometry: fills (both rules), strokes, dashes, Bézier fills and round joins — the same
/// image from both rasterizers. Observed delta ≈ 0.7 (edge coverage quantization; under
/// matched gs policy the exported program is pixel-identical to the source PDF's).
#[test]
fn geometry_page_matches_ghostscript() {
    assert_matches_oracle("geometry", &geometry_pdf());
}

/// The DCTDecode image at a 1:1 pixel mapping: same JPEG bytes, two IDCTs. Observed
/// delta ≈ 0.003 (JPEG decoder rounding on a handful of samples).
#[test]
fn image_page_matches_ghostscript() {
    assert_matches_oracle("image", &image_pdf());
}

/// The CCITTFaxDecode passthrough, with and without `/BlackIs1 true`: the same fax bytes
/// through PDF's and PostScript's decoders. The flag inverts the decoded polarity on both
/// sides identically (measured: PdfCraft's raster and gs's rasterization of the export are
/// pixel-equal in both configurations), so the passthrough's identity `/Decode` is the
/// faithful choice — this test pins that no compensating `/Decode [1 0]` sneaks in.
#[test]
fn fax_page_matches_ghostscript() {
    assert_matches_oracle("fax-black", &fax_pdf(false));
    assert_matches_oracle("fax-blackis1", &fax_pdf(true));
    let out = export(Arc::new(fax_pdf(true)), None, &[0], PsFormat::Ps).expect("exports");
    let ps = String::from_utf8_lossy(&out.bytes);
    assert!(ps.contains("/BlackIs1 true"), "the flag itself passes through: {ps}");
    assert!(ps.contains("/Decode [0 1]"), "the identity decode passes through: {ps}");
    assert!(!ps.contains("/Decode [1 0]"), "no compensating inversion: {ps}");
}

/// Text over every Layer A strategy: Type 1 as `show` (gs's FreeType against skrifa's
/// outlines of the same box charstrings), embedded TrueType and a non-embedded font as
/// vectorized outlines. Observed delta ≈ 0.2.
/// Text over the Layer A/B strategies: Type 1 as `show` (gs's FreeType against skrifa's
/// outlines of the same box charstrings), embedded TrueType as Type 42 `show` (gs resolving
/// the assigned codes through the font's `/Encoding` names and `/CharStrings`), and a
/// non-embedded font as vectorized outlines. Observed delta ≈ 0.2.
#[test]
fn text_page_matches_ghostscript() {
    assert_matches_oracle("text", &text_pdf());
}

/// Decrypt the eexec section of an exported Type 1 program: the one hex line
/// after `currentfile eexec`, eexec-decrypted. The decryptor advances its
/// seed by the *ciphertext* byte (the input), so it is not the fixture
/// builder's encryptor run backwards — the two share only the XOR. Test-side
/// expect on the markers: the export always emits the Layer A shape.
fn t1_eexec_plain(ps: &[u8]) -> Vec<u8> {
    const MARK: &[u8] = b"currentfile eexec\n";
    let at = ps.windows(MARK.len()).position(|w| w == MARK).expect("the eexec marker");
    let hex_start = at + MARK.len();
    let hex_end = hex_start + ps[hex_start..].iter().position(|&b| b == b'\n').expect("the hex line ends");
    let cipher: Vec<u8> =
        ps[hex_start..hex_end].chunks(2).map(|pair| u8::from_str_radix(&String::from_utf8_lossy(pair), 16).expect("a hex pair")).collect();
    let mut seed: u32 = 55665;
    cipher
        .iter()
        .map(|&c| {
            let plain = c ^ ((seed >> 8) as u8);
            seed = (u32::from(c).wrapping_add(seed).wrapping_mul(52845).wrapping_add(22719)) & 0xFFFF;
            plain
        })
        .collect()
}

/// Whether `haystack` contains `needle` (byte-wise).
fn has_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Task 8's gate: a subsetted Type 1 program loads in gs. A page drawing only
/// 'A' exports with a program whose charstrings are `.notdef` and A alone —
/// proven by decrypting the emitted eexec (B's charstring is gone) — gs's
/// txtwrite reads the drawn string back out of the exported PostScript, and
/// the whole-page rasters match (Task 6's helper set). Skips its gs halves
/// when gs is not installed.
#[test]
fn subset_program_loads_in_gs() {
    let pdf = assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        // F1: the embedded Type 1 (descriptor 5, program 6), 'A' alone drawn.
        b"<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftTest /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>".to_vec(),
        stream_obj("", b"BT /F1 72 Tf 14.4 100 Td (A) Tj ET"),
        b"<< /Type /FontDescriptor /FontName /PdfCraftTest /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 6 0 R >>".to_vec(),
        stream_obj("", &t1_program()),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]);
    let out = export(std::sync::Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the Type 1 page exports");
    assert!(out.warnings.is_empty(), "subsetting degrades nothing: {:?}", out.warnings);
    // The program really was subsetted: A's charstring rides, B's does not.
    let plain = t1_eexec_plain(&out.bytes);
    assert!(has_bytes(&plain, b"/A 20 RD "), "A's charstring rides: {plain:?}");
    assert!(!has_bytes(&plain, b"/B "), "B's charstring is dropped: {plain:?}");
    assert!(has_bytes(&plain, b"/.notdef "), ".notdef rides: {plain:?}");
    let Some(text) = gs_text(&out.bytes, "t1-subset") else {
        return;
    };
    assert!(text.contains('A'), "the drawn string is extractable: {text:?}");
    assert_matches_oracle("t1-subset", &pdf);
}

/// Real-font TrueType (Inter) as a live Type 42 font: gs's FreeType renders the
/// embedded sfnts through the exported `/FontType 42` dictionary (the
/// `fonts.rs` module docs record the emission shape this gs build requires),
/// and the raster must match PdfCraft's own render of the same page. Observed
/// delta ≈ 1.4 (FreeType hinting against vello analytic AA on the densest
/// edge coverage in the corpus). Skips (with a message) if the asset is
/// absent.
#[test]
fn real_truetype_page_matches_ghostscript() {
    let Some(pdf) = inter_pdf() else {
        eprintln!("skipping the real-font oracle fixture: assets/fonts/Inter-Regular.ttf is absent");
        return;
    };
    assert_matches_oracle("real-truetype", &pdf);
}

/// The W1 merge gate (real font, extraction half): embedded Inter leaves as a
/// Type 42 font — `/FontType 42` in the program, `show`-shaped text on the
/// page, no degradation note — and the drawn text is recoverable: gs's
/// txtwrite reads `HAMBURGEFONSTIV` back out of the exported PostScript,
/// proving the `/Encoding` and `/CharStrings` names carry the glyphs' semantic
/// identities (the ps2write shape). The raster halves of the gate are
/// [`real_truetype_page_matches_ghostscript`] (parity, real Inter) and
/// [`text_page_matches_ghostscript`] (the strategy mix). Skips when the asset
/// is absent or gs is not installed.
#[test]
fn embedded_real_truetype_draws_and_extracts_as_type42() {
    let path = format!("{}/../../assets/fonts/Inter-Regular.ttf", env!("CARGO_MANIFEST_DIR"));
    let Ok(inter) = std::fs::read(&path) else {
        eprintln!("skipping the real-font extraction gate: assets/fonts/Inter-Regular.ttf is absent");
        return;
    };
    let pdf = assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 400 200] >>".to_vec(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Inter-Regular /Encoding /WinAnsiEncoding /FontDescriptor 5 0 R >>".to_vec(),
        stream_obj("", b"BT /F1 12 Tf 20 100 Td (HAMBURGEFONSTIV) Tj ET"),
        b"<< /Type /FontDescriptor /FontName /Inter-Regular /Flags 32 /FontBBox [0 -200 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>".to_vec(),
        stream_obj("", &inter),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]);
    let out = export(Arc::new(pdf), &[0], PsFormat::Ps).expect("real-font page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("/FontType 42"), "a Type 42 dictionary is emitted: {s}");
    assert!(s.contains("xyshow"), "the text draws through the font, not vectorized paths: {s}");
    assert!(out.warnings.iter().all(|n| !n.text.contains("Type 42 pending cmap synthesis")), "no pending-synthesis note: {:?}", out.warnings);
    let Some(text) = gs_text(&out.bytes, "inter-extract") else {
        return;
    };
    assert!(text.contains("HAMBURGEFONSTIV"), "the drawn text is extractable: {text:?}");
}

/// Axial and radial shadings: gs's `shfill` interpolation against the raster renderer's
/// gradients. Observed delta ≈ 0.1 (gradient interpolation differs by design).
#[test]
fn shading_page_matches_ghostscript() {
    assert_matches_oracle("shading", &shading_pdf());
}

/// EPS output of the same geometry page rasterizes identically (gs runs the file and a
/// trailing `showpage` ejects the page the importing application would own). Observed
/// delta ≈ 0.7, the geometry fixture's.
#[test]
fn eps_output_raster_matches_ghostscript() {
    let pdf = geometry_pdf();
    let out = export(Arc::new(pdf.clone()), None, &[0], PsFormat::Eps).expect("the fixture exports as EPS");
    let base = std::env::temp_dir().join(format!("pdfcraft-ps-oracle-{}-eps", std::process::id()));
    let eps_path = base.with_extension("eps");
    let png_path = base.with_extension("png");
    std::fs::write(&eps_path, &out.bytes).expect("writing the oracle's temp .eps cannot fail");
    let run = Command::new("gs")
        .args(gs_args(&png_path))
        // The EPS body never runs `showpage` (the importer owns the page), so the harness
        // ejects it after the file: run `file.eps`, then `showpage`.
        .arg("-f")
        .arg(&eps_path)
        .arg("-c")
        .arg("showpage")
        .output();
    let output = match run {
        Ok(output) => output,
        Err(why) => {
            eprintln!("skipping the EPS oracle fixture: gs could not be spawned ({why})");
            return;
        }
    };
    assert!(output.status.success(), "gs rejected the exported EPS program:\n{}", String::from_utf8_lossy(&output.stderr));
    let png = std::fs::read(&png_path).expect("gs wrote its output PNG");
    let _ = std::fs::remove_file(&eps_path);
    let _ = std::fs::remove_file(&png_path);
    let gs = decode_gs_png(&png, "eps");
    let mine = pdfcraft_raster(&pdf);
    assert_eq!((gs.0, gs.1), (mine.0, mine.1), "the two rasters of eps cover the same canvas");
    let delta = mean_channel_delta(&gs.2, &mine.2);
    eprintln!("eps: mean per-channel delta {delta:.3}/255 (tolerance {TOLERANCE})");
    assert!(delta < TOLERANCE, "EPS: gs and PdfCraft rasters diverge ({delta:.3} ≥ {TOLERANCE})");
}

/// Transparency degrades to a note (Layer A policy; flattening is Layer C): the export
/// completes, both draws emit and the degradation is recorded — never raster-compared.
#[test]
fn transparency_page_exports_with_note() {
    let out = export(Arc::new(transparency_pdf()), None, &[0], PsFormat::Ps).expect("transparent content exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert_eq!(s.matches("fill\n").count(), 2, "both draws still emit: {s}");
    assert!(out.warnings.iter().any(|n| n.text.contains("transparency ignored (blend Multiply)")), "the degradation is recorded: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency ignored (blend Multiply)\n"), "note embedded: {s}");
}

/// A degraded shading paints ink: the oversized-mesh fixture degrades to the flat
/// mid-tone, and the exported fallback's `fill` must cover the page in gs's raster (the
/// pre-fix `clip`-then-`fill` fallback painted nothing — gs-verified: 0 inked pixels).
/// The bytes are checked unconditionally (a `fill` that follows a rebuilt `newpath…`
/// sequence), the raster wherever gs is installed.
#[test]
fn degraded_shading_paints_ink() {
    let out = export(Arc::new(degraded_shading_pdf()), None, &[0], PsFormat::Ps).expect("the fixture exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(!s.contains("shfill"), "no shading dictionary painted: {s}");
    assert!(
        out.warnings.iter().any(|n| n.text.contains("shading approximated") && n.text.contains("string limit")),
        "the degradation is recorded: {:?}",
        out.warnings
    );
    let path = s.find("\nnewpath\n").expect("the fallback rebuilds the path");
    let fill = path + s[path..].find("\nfill\n").expect("the rebuilt path is filled");
    assert!(path < fill, "the fill follows the rebuilt newpath… sequence: {s}");
    let Some((_, _, rgb)) = gs_raster(&out.bytes, "degraded-shading") else {
        return;
    };
    // The flat mid-tone is ~0.5 gray over the whole page box: most of the raster is ink.
    let inked = rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 200).count();
    assert!(inked > (PX.0 * PX.1 / 2) as usize, "the degraded shading painted {inked} inked pixels, not a blank region");
}

/// The W2 CFF-promotion merge gate (synthetic gs proof): a simple font's
/// embedded CFF (`/FontFile3 /Type1C`) leaves as a PS Type 0 composite — the
/// ReadData loader route, a `cidrange`/`cidchar` CMap, a `composefont`
/// parent — and both rasterizers draw the same glyphs. The page embeds the
/// craft-fonts Latin face when the checkout carries a bare-CFF one (read at
/// test runtime via [`craft_face`]); otherwise the synthetic [`cff_program`]
/// covers the arm with a skip line for the checkout half (today's checkout
/// holds only TrueType faces, so CI without it runs the synthetic arm).
///
/// **Extraction is deliberately not asserted — a measured correction of the
/// gate spec.** The spec expected `gs_text` to return the drawn Latin string
/// "through the face's charset SIDs", but on this oracle build (gs 10.08.0)
/// txtwrite maps composite text to its raw 2-byte codes as Unicode: the
/// Task 5 probes return `U+0001 U+0002` for codes `<0001> <0002>` shown
/// through the promoted font, unchanged when the CMap's mapping is swapped,
/// and the charset's own names ('A', 'B') never surface. A promoted CFF is a
/// CID arm the moment it is emitted, so Task 3's finding (extraction is
/// meaningful only where the emitted font carries `/Encoding` glyph names —
/// the base-font arms) covers it too. The gate therefore proves the arm the
/// stronger way the plan scopes for CID arms — structure + oracle ink +
/// raster parity ([`assert_matches_oracle`]) — while extraction stays proven
/// where names exist, by the Type 42 gate
/// ([`embedded_real_truetype_draws_and_extracts_as_type42`]). Skips its gs
/// half when gs is not installed.
#[test]
fn cff_promotion_page_matches_ghostscript_and_extracts() {
    let (program, from_checkout) = match craft_face(&["Latn"]) {
        Some((bytes, FaceFlavor::Cff)) => (bytes, true),
        Some((_, flavor)) => {
            eprintln!(
                "skipping the checkout half of the CFF gate: the Latin face is {flavor:?}, not a bare CFF; the synthetic program covers the promotion arm"
            );
            (cff_program(), false)
        }
        None => (cff_program(), false),
    };
    let pdf = assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        // F1: the simple font whose FontFile3 promotes (descriptor 5, program 6).
        b"<< /Type /Font /Subtype /Type1 /BaseFont /PdfCraftCFF /FirstChar 65 /LastChar 66 /Widths [600 700] /Encoding /StandardEncoding /FontDescriptor 5 0 R >>".to_vec(),
        stream_obj("", b"BT /F1 72 Tf 14.4 100 Td (ABAB) Tj ET"),
        b"<< /Type /FontDescriptor /FontName /PdfCraftCFF /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>".to_vec(),
        stream_obj("/Subtype /Type1C", &program),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]);
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the CFF page exports");
    assert!(out.warnings.is_empty(), "the promotion arm degrades nothing: {:?}", out.warnings);
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    // Structure — the ratified Task 5 shapes: gs's own CFF loader route, the
    // cidrange/cidchar CMap grammar with integer CID operands (`bfrange`/
    // `bfchar` is the ToUnicode grammar: it defines but maps nothing — the
    // probes inked zero through it), the composefont parent, and text drawing
    // through the composite rather than vectorized paths.
    assert!(s.contains("/ASCIIHexDecode filter //true //true ReadData\n"), "gs's own CFF loader route: {s}");
    assert!(s.contains("composefont pop\n"), "the composefont parent: {s}");
    assert!(s.contains("begincidrange") || s.contains("begincidchar"), "the CMap body uses the cid grammar: {s}");
    assert!(!s.contains("bfchar") && !s.contains("bfrange"), "the ToUnicode grammar maps no code in a CID CMap: {s}");
    assert!(s.contains("xyshow"), "the text draws through the composite: {s}");
    if !from_checkout {
        // The synthetic font's codes are the identity gids 1 and 2, which the
        // CMap collapses into one cidrange line.
        assert!(s.contains("<0001> <0002> 1\n"), "identity code==cid mappings collapse to one cidrange: {s}");
    }
    // Ink on the oracle side first: a CMap that maps nothing renders blank,
    // and two blank rasters would delta 0 and pass the comparison vacuously.
    let Some((_, _, rgb)) = gs_raster(&out.bytes, "cff-promote") else { return };
    let inked = rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 200).count();
    // Measured ≈ 740 inked pixels for the synthetic arm (the curve of A and
    // the box of B twice at 72 pt); the threshold keeps headroom below that
    // while sitting far above zero, which is what a CMap that maps nothing
    // paints (the raster comparison below catches wrong-but-nonempty glyphs).
    assert!(inked > 200, "the promoted composite painted {inked} inked pixels — a CMap or charset mismatch renders blank");
    assert_matches_oracle("cff-promote", &pdf);
}

/// The W2 CID merge gate (craft-fonts at runtime): a Type0/CIDFontType2 page
/// drawing CJK text (日本語) with the checkout's Jpn face, compared through the
/// whole oracle loop. [`craft_face`] reads the face where the checkout stands;
/// its cmap picks the gids (under `/Identity-H` with an identity
/// `/CIDToGIDMap` the PDF's 2-byte codes are CIDs are gids), so the gate
/// exercises the full chain: hayro's embedded-TrueType rendering of the source
/// PDF against gs's type-11 CIDFont rendering of the exported composite.
///
/// **Extraction is deliberately not asserted — the plan's own scoping, on this
/// build's semantics.** CID arms carry no glyph names, and txtwrite maps their
/// text to the raw codes as Unicode (see
/// [`cff_promotion_page_matches_ghostscript_and_extracts`] for the probes), so
/// identity-CID CJK text has no recoverable Unicode through any name route:
/// the extracted bytes would be the codes, not the characters. CJK fidelity is
/// proven the stronger way instead — structural shape + oracle ink + raster
/// match — while extraction coverage lives with the Latin arms that carry
/// names (the Type 42 gate). The gate skips (with a message) when
/// `CRAFT_FONTS_DIR` is unset, the Jpn face is absent, or it is not a
/// TrueType face (only that flavor rides the CIDFontType2 arm here: the bare
/// CFF flavors have no char→CID route without an ordering CMap). Skips its gs
/// half when gs is not installed.
#[test]
fn cid_cjk_page_matches_ghostscript() {
    let Some((bytes, flavor)) = craft_face(&["Jpn"]) else { return };
    let FaceFlavor::TrueType = flavor else {
        eprintln!("skipping the CID CJK gate: the Jpn face is {flavor:?}; only the TrueType flavor rides the CIDFontType2 arm");
        return;
    };
    use skrifa::MetadataProvider as _;
    let Ok(font) = skrifa::FontRef::new(&bytes) else {
        eprintln!("skipping the CID CJK gate: the Jpn face does not parse as an sfnt");
        return;
    };
    let charmap = font.charmap();
    // A gid past 16 bits cannot ride the composite's 2-byte codes, so it drops.
    let codes: Vec<u16> = "日本語".chars().filter_map(|c| charmap.map(c)).map(|g| g.to_u32()).filter_map(|gid| u16::try_from(gid).ok()).collect();
    if codes.len() < 2 {
        eprintln!("skipping the CID CJK gate: the Jpn face covers only {} of 日本語", codes.len());
        return;
    }
    let show: String = codes.iter().map(|c| format!("{c:04X}")).collect();
    let pdf = assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [8 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        // F1: the Type0 parent over one CIDFontType2 descendant (7), whose
        // descriptor (5) embeds the face (6). Identity-H + identity CIDToGIDMap:
        // the content's codes are CIDs are gids.
        b"<< /Type /Font /Subtype /Type0 /BaseFont /CraftJpn /Encoding /Identity-H /DescendantFonts [7 0 R] >>".to_vec(),
        stream_obj("", format!("BT /F1 36 Tf 14.4 200 Td <{show}> Tj ET").as_bytes()),
        b"<< /Type /FontDescriptor /FontName /CraftJpn /Flags 4 /FontBBox [0 -200 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>".to_vec(),
        stream_obj("", &bytes),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /CraftJpn /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 /CIDToGIDMap /Identity >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]);
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the CJK page exports");
    assert!(out.warnings.is_empty(), "the CIDFontType2 arm degrades nothing: {:?}", out.warnings);
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    // Structure — pdftops' type-11 key set around the PDF-transliterated ones,
    // the composefont parent, and text drawing through the composite.
    assert!(s.contains("/CIDFontType 2 def\n"), "the type-11 CIDFont dict: {s}");
    assert!(s.contains("/CIDToGIDMap /Identity def\n"), "the identity CID-to-GID mapping: {s}");
    assert!(s.contains("/GDBytes 2 def\n"), "gs's type-11 key set: {s}");
    assert!(s.contains("/CIDMap "), "the identity CIDMap: {s}");
    assert!(s.contains("composefont pop\n"), "the composefont parent: {s}");
    assert!(s.contains("xyshow"), "the text draws through the composite, not vectorized paths: {s}");
    // Ink on the oracle side first (a vacuous 0-delta pass needs it), then the
    // full raster comparison — the CJK fidelity proof this gate exists for.
    let Some((_, _, rgb)) = gs_raster(&out.bytes, "cid-cjk") else { return };
    let inked = rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 200).count();
    // Measured ≈ 1300 inked pixels for a stroke-dense synthetic Jpn face at
    // 36 pt; the threshold keeps headroom below that while sitting far above
    // zero, which is what a CMap or CIDMap that maps nothing paints.
    assert!(inked > 400, "the CIDFontType2 composite painted {inked} inked pixels — a CMap or CIDMap mismatch renders blank");
    assert_matches_oracle("cid-cjk", &pdf);
}
