//! The Ghostscript oracle: export → gs raster → compare against PdfCraft's own raster.
//!
//! Layer A's correctness claim is measured here, not asserted: for every fixture class the
//! exported PostScript is rasterized by Ghostscript (`png16m` at 50 dpi, the clean-room
//! external-process oracle the repo already uses for qpdf/CUPS-style gates) and the source
//! PDF by `pdfcraft_render` at the same effective dpi, and the mean per-channel delta of
//! the two rasters must sit under [`TOLERANCE`]. gs runs as an observed oracle process,
//! never linked; where it is not installed the raster fixtures skip with a message (the
//! CUPS-test precedent). Transparency is flattened (Layer C, Waves 2–3: opacity
//! groups and soft masks, then blend modes and the interaction gates — nested
//! groups, a tiling pattern, an image and a text run inside one group, and a
//! shading pattern with its `/BBox` clip): a real-transparency group's region —
//! sub-unit opacity, a soft mask, or a non-Normal blend mode — leaves as one
//! opaque 300-dpi raster (the blend composites inside the scene through vello),
//! and those pages enter the raster comparison too — under their own
//! [`FLATTEN_TOLERANCE`], because gs resamples the region raster while
//! `pdfcraft_raster` renders the source vectors.
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
/// corpus is ≈ 1.4 (real-font TrueType — the Inter Type 42 gate, where FreeType hinting
/// meets vello analytic AA on the densest edge coverage). The value keeps ≈ 1.4× headroom
/// over that worst case — enough to stay stable across gs builds, tight enough that a
/// mis-emitted operator (wrong colour, dropped draw, misplaced box) fails loudly: any of
/// those moves whole regions by tens of levels per channel.
const TOLERANCE: f64 = 2.0;
/// Mean per-channel delta, in 0–255 byte levels, below which the CMYK fixture's two rasters
/// count as the same image. The fixture needs its own bound because the two rasterizers
/// convert CMYK to RGB through *different* pipelines — gs renders `setcmykcolor` through its
/// built-in device conversion, PdfCraft's raster converts the source's CMYK through its
/// characterization profile — so every `k`/`K` region carries a constant per-channel offset
/// between the two, not just edge-quantization noise. On gs 10.08.0 the two pipelines land
/// within 2 levels per channel on every flat region and the measured whole-page delta is
/// ≈ 0.5 (the offsets are tiny); the value keeps ≈ 3× headroom over that measurement to
/// absorb a couple more levels of cross-build conversion shift — while staying tight enough
/// to fail loudly on a mis-emitted colour: a swapped CMYK pair was measured at ≈ 33, and a
/// dropped fill or wrong component moves whole regions by tens of levels.
const CMYK_TOLERANCE: f64 = 1.5;
/// Mean per-channel delta, in 0–255 byte levels, below which a *flattened*
/// transparency fixture's two rasters count as the same image (the Wave 2 gates:
/// the `/ca 0.5` group page and the `/SMask` alpha-mask page; the Wave 3 gates:
/// the `/BM /Multiply` group page, the nested-groups page, the tiling-in-group
/// page, the image+glyph-in-group page and the shading-in-group page). A flattened
/// region
/// is a photograph: the exporter replays the page at 300 dpi and emits it as one
/// opaque raster, which gs *resamples* onto the oracle's 50-dpi grid (nearest by
/// default — the emitted dictionaries carry no `/Interpolate`), while
/// `pdfcraft_raster` renders the source vectors at that grid directly. The
/// asymmetry is why this is its own bound and not [`TOLERANCE`]: the whole region
/// is gs's resample of a raster, not its rasterization of geometry. On gs 10.08.0
/// every flat-region fixture measures a stable 0.000 across repeated runs — their
/// geometry sits on the oracle's 1.44 pt pixel grid and their fills are flat, so a
/// 6:1 nearest-neighbour downsample of the 300-dpi raster reproduces vello's
/// analytic coverage byte for byte (pixel-identical rasters, verified: zero
/// differing pixels on the soft-mask fixture; the tiling-in-group page's tiles
/// step on the same grid, 36 black of every 60 raster pixels) — and the one
/// smooth-gradient fixture (shading-in-group) measures 0.024: a gradient
/// resampled 6:1 nearest against a direct 50-dpi evaluation differs by the
/// gradient's slope over a sub-pixel, not byte-for-byte like the flat regions.
/// The value keeps a small absolute
/// margin over those measurements for a build that interpolates images
/// differently (a level or two per channel on resampled edges) — while staying
/// far below a wrong
/// flattening: a mask recorded in the wrong device space (the bug this gate
/// caught; the mask's ink landed unscaled in the renderer's corner and masked the
/// whole draw out) measured ≈ 44.9, a shading clip rebased through the new full
/// matrix instead of `pre` alone (the review bug the shading gate caught: the
/// captured matrix applied twice, the clip box doubled and shifted) ≈ 37.2, a
/// blend dropped from the replay's group layer
/// (the non-vacuity probe for the blend gate) measured ≈ 21.4, a tiling pattern
/// recorded without the 300-dpi re-root (the bug the tiling-in-group gate caught:
/// tiles at one pixel per *point*, the wrong size and phase) measured ≈ 11.9, a
/// nested region cropped to the innermost frame only (the probe for the
/// nested-groups gate: the inner group's protruding fill missing) ≈ 12.0, and a
/// dropped mask, opacity or tile set moves whole regions by tens of levels per
/// channel.
const FLATTEN_TOLERANCE: f64 = 1.5;

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

/// A flat-paint page in the source's own spaces: two DeviceCMYK `k` fills, a DeviceGray
/// `g` fill, a DeviceRGB `rg` fill, a DeviceCMYK `K` stroke and two mixed-component `k`
/// fills — the `setcmykcolor`/raw-`setgray` emission surface (the regions stay disjoint,
/// so each colour's conversion difference stands alone in the delta).
fn cmyk_pdf() -> Vec<u8> {
    one_page(
        "",
        b"0 1 1 0 k\n20 20 100.8 72 re f\n\
          1 0 1 0 k\n158.4 14.4 115.2 86.4 re f\n\
          0.75 g\n14.4 115.2 115.2 57.6 re f\n\
          1 0 0 rg\n158.4 115.2 115.2 57.6 re f\n\
          0 0 1 0 0.2 K\n3 w\n14.4 216 m 273.6 216 l S\n\
          0.2 0.4 0.6 0.1 k\n43.2 244.8 115.2 100.8 re f\n\
          0 0.5 0.3 0.3 k\n158.4 244.8 115.2 100.8 re f",
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

/// A page that sets `/ca 0.5` and then draws a form XObject carrying a real
/// transparency group: the interpreter forwards the half opacity into the group
/// push, so the exporter withholds the form's vector bytes and flattens the region
/// into one opaque 300-dpi raster (Wave 2). The group's two half-opacity fills
/// overlap, so the flattened raster must composite *stacked* alpha exactly as
/// `page_render` composites it, and the black backdrop beneath the group proves the
/// raster carries the content it covers (the replay log repaints the backdrop under
/// the group's region). A blend mode outside any group keeps its note-only policy —
/// the `lib.rs` unit gate covers that edge rule; this fixture is the flattening arm.
fn transparency_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"0 0 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg 43.2 172.8 201.6 158.4 re f\n1 0 0 rg 43.2 43.2 201.6 158.4 re f",
        ),
    ])
}

/// A page drawing a fill through an `/SMask` alpha mask: the ExtGState's `/G` form
/// (itself a transparency group) paints opaque black over exactly half the draw's
/// width — x 28.8–144 of the draw's 28.8–259.2 — so the masked blue fill shows its
/// left half only. The mask is taken from the graphics state at the form entry and
/// forwarded into the group push, so the region flattens into one raster whose
/// replay draws the mask through hayro's own `draw_soft_mask` — the same pixels
/// `page_render` composites. The yellow backdrop beneath makes the mask's effect
/// (and its absence, yesterday's note-only behaviour) visible either way: with the
/// mask dropped the blue would cover the whole rect and the raster comparison
/// would fail by tens of levels.
fn soft_mask_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 72 259.2 216 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        b"<< /Type /ExtGState /SMask << /S /Alpha /G 7 0 R >> >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg 28.8 100.8 230.4 158.4 re f",
        ),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 g 28.8 0 115.2 360 re f",
        ),
    ])
}

/// A page drawing a form XObject under a `/BM /Multiply` ExtGState: the form carries
/// a real transparency group, so the blend the interpreter forwards into the group
/// push composites the group's content — one blue fill spanning both backdrop bands —
/// onto the backdrop with Multiply, and the exporter flattens the group into one
/// opaque 300-dpi raster whose pixels are that composite. The yellow band makes the
/// blend non-optional: Multiply turns the blue fill black there (1,1,0)×(0,0,1),
/// where a dropped blend would leave the blue covering the yellow — a whole-band,
/// tens-of-levels failure the raster comparison catches (the cyan band multiplies
/// to blue either way and carries no signal). No `transparency ignored (blend …)`
/// note may fire: the page's own backdrop draws are announced at `Normal`, and the
/// `Multiply` set by `/GS1 gs` rides the form's group push, whose raster carries it
/// (the edge rule's in-group side; the note side — a blend announced with no open
/// real group — is `lib.rs::blend_mode_produces_note_not_ops` and the `device.rs`
/// unit gate).
fn blend_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 158.4 re f\n0 1 1 rg 14.4 187.2 259.2 158.4 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /BM /Multiply >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg 43.2 43.2 201.6 273.6 re f",
        ),
    ])
}

/// A page drawing **nested** real-transparency groups: a backdrop band, then a form
/// (`/ca 0.5` at entry) whose own content paints a blue fill and then draws a second
/// form under `/ca 0.9` whose content paints a red fill that **protrudes** beyond the
/// blue box (over the white page on the right, x 230.4–273.6). The inner group's pop
/// flattens its own region and its raster lands in the *outer* group's withheld
/// buffer; the outer's pop re-flattens the **union** — `extend` widens every open
/// frame, so the outer's frame covers the inner's protruding red — and discards the
/// withheld bytes (inner raster included): the final stream carries exactly the
/// outer's one raster, placed over the union region. The black band beneath the blue
/// fill proves the raster repaints the backdrop under the region, and the white page
/// beneath the protrusion makes a region that missed it fail loudly — a raster
/// cropped to the blue box alone would leave the 0.45-opacity red missing over white
/// (a whole-band, tens-of-levels failure; see the gate's doc for the probe that
/// measured it).
fn nested_groups_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"0 0 0 rg 28.8 57.6 201.6 244.8 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /ExtGState << /GS2 7 0 R >> /XObject << /Fm2 8 0 R >> >>",
            b"0 0 1 rg 43.2 72 172.8 216 re f\nq /GS2 gs /Fm2 Do Q",
        ),
        b"<< /Type /ExtGState /ca 0.9 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"1 0 0 rg 187.2 43.2 86.4 288 re f",
        ),
    ])
}

/// A page painting a W1 tiling pattern inside an opacity group — and, on the same
/// page, the combination Task 12 deferred: **entry-state `/ca 0.5` with an in-form
/// `/BM`**. The form (real via the entry `/ca 0.5`) first paints a red fill, then —
/// under `/GS2` (`/BM /Multiply`, announced *inside* the open real group, where the
/// edge rule correctly notes nothing) — fills the same rect through the Task 4
/// tiling pattern (one blue square per 14.4 pt cell). The whole form's vector bytes
/// (both fills *and* the `makepattern setpattern` use site) are withheld and
/// discarded; the flattened raster carries the multiply: the blue tiles multiply
/// against the red under-fill to black squares inside the isolated group, then the
/// group composites at half opacity over the yellow backdrop. A dropped blend would
/// paint blue squares instead — a tens-of-levels failure. The pattern resource
/// itself is interned into the prolog at draw time and may still ride there
/// unreferenced — harmless legal PostScript, disclosed by the gate.
fn tiling_inside_group_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /ExtGState << /GS2 7 0 R >> /Pattern << /P1 8 0 R >> >>",
            b"1 0 0 rg 28.8 43.2 230.4 273.6 re f\nq /GS2 gs /Pattern cs /P1 scn 28.8 43.2 230.4 273.6 re f Q",
        ),
        b"<< /Type /ExtGState /BM /Multiply >>".to_vec(),
        stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 14.4 14.4] /XStep 14.4 /YStep 14.4 /Resources << >>",
            b"0 0 1 rg 2.88 2.88 8.64 8.64 re f",
        ),
    ])
}

/// A page drawing a JPEG image and an embedded-TrueType text run inside one opacity
/// group: the form (real via `/ca 0.5` at entry) draws the `image_pdf` JPEG 1:1 at
/// the oracle grid (`/Im1`, DCTDecode, XObject — recorded by value, no hole), then a
/// green `(AB)` text run through the synthetic TrueType (`ttf_program`, a Type 42
/// `xyshow` run — the font dict rides the page resources so the device's table knows
/// it and the run batches instead of vectorizing; the same object is on the form's
/// resources for the interpreter's resolution). The text run stays pending across
/// the image draw and flushes at the form's `/BBox` clip pop — still inside the
/// group — into the **withheld** buffer, where the raster replaces it: if the flush
/// landed outside (the bug class this gate pins), the `xyshow` and the JPEG's DCT
/// invocation would leak into the final program and the byte checks catch it. The
/// glyph boxes (x 21.6–43.2 and 64.8–86.4, y 100.8–129.6) sit partly over the image
/// (x 28.8–172.8, y 72–252), so the raster must composite draw order — glyphs over
/// image — exactly as `pdfcraft_raster` composites the source.
fn image_and_glyph_inside_group_pdf() -> Vec<u8> {
    let jpeg = jpeg_gradient(100, 125);
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> /Font << /F1 7 0 R >> >> >>".to_vec(),
        stream_obj("", b"0 0 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /XObject << /Im1 8 0 R >> /Font << /F1 7 0 R >> >>",
            b"q 144 0 0 180 28.8 72 cm /Im1 Do Q\n0 1 0 rg\nBT /F1 72 Tf 14.4 100.8 Td (AB) Tj ET",
        ),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /PdfCraftTT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 9 0 R >>".to_vec(),
        stream_obj("/Type /XObject /Subtype /Image /Width 100 /Height 125 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", &jpeg),
        b"<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 10 0 R >>".to_vec(),
        stream_obj("", &ttf_program()),
    ])
}

/// A page painting a shading **pattern** inside an opacity group — the end-to-end
/// proof of the review fix for `Pattern::re_rooted`'s shading arm: the pattern
/// carries **both** a non-identity `/Matrix` (doubling, translating to 28.8, 43.2)
/// and a shading `/BBox` (which is what becomes the paint's clip — without either
/// the gate would pass vacuously). The gradient runs red→blue along shading x 0–72
/// → page x 28.8–172.8; the `/BBox` clip [0 0 72 129.6] cuts it to x 28.8–172.8,
/// y 43.2–302.4, while the fill rect runs to x 259.2 and y 316.8 — so the right
/// band and the top strip of the fill must show the yellow backdrop, exactly where
/// a double-transformed clip (the pre-fix `re_rooted` re-derived the stored clip
/// through the new full matrix, applying the captured matrix twice: the clip box
/// became x 86.4–374.4, y 129.6–647) would let the extended blue tail paint on.
/// The form's group (real via the entry `/ca 0.5`) withholds the `shfill` route
/// and the flatten raster carries the gradient — at 300-dpi density through the
/// re-rooted matrix, clipped through the pre-only-rebased clip.
fn shading_inside_group_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /Pattern << /P1 7 0 R >> >>",
            b"/Pattern cs /P1 scn 28.8 43.2 230.4 273.6 re f",
        ),
        b"<< /PatternType 2 /Matrix [2 0 0 2 28.8 43.2] /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 72 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> /BBox [0 0 72 129.6] /Extend [true true] >> >>".to_vec(),
    ])
}

// **Hostile-geometry fixtures (Task 14, the never-crash proofs).** The four
// fixtures below are deliberately hostile — off-the-scale coordinates, a
// non-finite `/BBox`, a self-referencing tiling cell, more real groups than
// the flatten budget — and none of them is raster-compared: their whole point
// is *graceful decline or disclosed flattening*, so the gates assert
// completion, balance and the note set, never pixel parity.

/// A real-transparency group (`/ca 0.5` at the form's entry) whose only draw
/// sits at ~1e7 pt, entirely outside the 288×360 page: the frame's box is huge
/// but finite, so no cap misfires — the region ∩ page is empty and the
/// flattener's empty-region contract applies (Task 8: the group painted
/// nothing on the page, so the withheld vector is discarded and the
/// flattening still discloses itself, rather than today's decline note the
/// task brief predicted — pinned as the implementation behaves; see the gate
/// `huge_group_bbox_flattens_to_an_empty_region`).
fn huge_group_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg\n10000000 10000000 100 100 re f",
        ),
    ])
}

/// 129 real-transparency groups on one page (`/GS1 gs` + the same group form
/// drawn 129 times): the per-page flatten budget ([`MAX_FLATTENS_PER_PAGE`],
/// 128) spends one flatten per pop, so the first 128 groups each leave as one
/// opaque raster and the 129th **declines** — its withheld vector bytes flush
/// back into the program with today's note. The never-silent proof: some
/// rasters, some vector, both notes, one balanced program.
fn groups_budget_pdf() -> Vec<u8> {
    let mut content = Vec::new();
    for _ in 0..129 {
        content.extend_from_slice(b"q /GS1 gs /Fm1 Do Q\n");
    }
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", &content),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg 20 20 30 30 re f",
        ),
    ])
}

/// A real-transparency group whose form carries a `/BBox` entry beyond the f32
/// range (1e40 digits, the `pattern.rs::hostile_tiling_geometry_degrades`
/// magnitude): the interpreter parses it to infinity and pushes the form's
/// clip with an infinite x1, which is the one route non-finite geometry
/// reaches the device through `export()` — content-stream operands that
/// overflow the parse are dropped by the interpreter before any draw (observed
/// in this task's probes), so the clip is what exercises the emitter's
/// non-finite repair. The draw itself is an ordinary on-page fill, so the
/// region is finite and the group flattens — the replay pushes the infinite
/// clip through vello under the panic guard, and the emitted clip path
/// coordinates are repaired (`0`, with the fidelity note).
fn non_finite_group_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 10000000000000000000000000000000000000000 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << >>",
            b"0 0 1 rg 20 20 100 100 re f",
        ),
    ])
}

/// The nested-budget interaction (Task 14): a tiling cell that names **itself**
/// in its own resources (the `pattern.rs::hostile_tiling_geometry_degrades`
/// self-reference shape), painted *inside* a real-transparency group. The
/// vendor's paint budget stops the recursion — `interpret` returns `None` at
/// the bound and every level above it collects a usable cell that fills
/// through the nested pattern — so the exporter interns a chain of pattern
/// resources in the prolog and notes the deepest decline ("cell undecodable")
/// while the fill declines to its flat stand-in *inside the withheld group
/// buffer*. The group's pop then flattens the region: the replay's renderer
/// meets the same budget refusal (the same machinery `page_render` uses, so
/// the raster composites exactly what the reference renders), the withheld
/// bytes — flat-gray fill included — are discarded, and the note survives
/// because fidelity notes accumulate globally, never in a withheld buffer.
fn self_referencing_cell_group_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream_obj("", b"1 1 0 rg 14.4 14.4 259.2 331.2 re f\nq /GS1 gs /Fm1 Do Q"),
        b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 288 360] /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /Pattern << /P1 7 0 R >> >>",
            b"/Pattern cs /P1 scn 28.8 43.2 230.4 273.6 re f",
        ),
        stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /Resources << /Pattern << /P1 7 0 R >> >>",
            b"/Pattern cs /P1 scn 1 1 8 8 re f",
        ),
    ])
}

/// A page of `/Separation` spot colours: three colorants over each alternate device
/// space (CMYK, RGB, Gray), each tint transform an exponential function, plus a
/// calculator transform whose outputs exceed its `/Range` (1.5 and -0.2 clamp to 1 and
/// 0) — the Task 3 colorspace-resource emission surface. The first colorant paints at
/// two tints, so one resource serves two use sites. The CMYK-alternate regions carry
/// the same cross-pipeline conversion difference as the CMYK fixture (see
/// [`CMYK_TOLERANCE`]), so the gate uses its bound.
fn separation_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj(
            "",
            b"/CS1 cs 1 sc 14.4 259.2 129.6 86.4 re f\n\
              /CS1 cs 0.35 sc 158.4 259.2 115.2 86.4 re f\n\
              /CS2 cs 0.75 sc 14.4 129.6 129.6 115.2 re f\n\
              /CS3 cs 0.5 sc 158.4 129.6 115.2 115.2 re f\n\
              /CS4 cs 0.5 sc 14.4 14.4 259.2 100.8 re f",
        ),
        b"<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << /ColorSpace << /CS1 [/Separation /Spot /DeviceCMYK 5 0 R] /CS2 [/Separation /Plate2 /DeviceRGB 6 0 R] /CS3 [/Separation /Ink3 /DeviceGray 7 0 R] /CS4 [/Separation /CalcSpot /DeviceCMYK 8 0 R] >> >> >>".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0.8 0 0 1] /N 1 >>".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [1 0.5 0] /C1 [0 0.5 1] /N 1 >>".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".to_vec(),
        stream_obj("/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1 0 1]", b"{ pop 1.5 0 -0.2 0 }"),
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
/// raster. `None` (after a skip message) when gs is not installed — unless
/// `PDFCRAFT_ORACLES_REQUIRED` is set, when the missing oracle fails the test
/// (crates/oracle's contract); a failed gs run fails the test with gs's own stderr —
/// the oracle exists to catch broken programs.
fn gs_raster(ps: &[u8], tag: &str) -> Option<(u32, u32, Vec<u8>)> {
    let base = std::env::temp_dir().join(format!("pdfcraft-ps-oracle-{}-{tag}", std::process::id()));
    let ps_path = base.with_extension("ps");
    let png_path = base.with_extension("png");
    std::fs::write(&ps_path, ps).expect("writing the oracle's temp .ps cannot fail");
    let run = Command::new("gs").args(gs_args(&png_path)).arg(&ps_path).output();
    let output = match run {
        Ok(output) => output,
        Err(why) => {
            let _ = std::fs::remove_file(&ps_path);
            // The shared oracle contract (crates/oracle): a missing oracle is a declared
            // skip or a failure, never a silent pass — CI sets the variable, so absence
            // breaks the build there and skips on a contributor's machine.
            assert!(
                std::env::var_os("PDFCRAFT_ORACLES_REQUIRED").is_none(),
                "gs is required (PDFCRAFT_ORACLES_REQUIRED is set) but could not be spawned: {why}"
            );
            eprintln!("skipping the {tag} oracle fixture: gs could not be spawned ({why})");
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
/// `tolerance` is the gate's own bound ([`TOLERANCE`], or [`CMYK_TOLERANCE`] for the CMYK
/// fixture — the two CMYK→RGB pipelines differ structurally, see its doc). Returns the
/// achieved mean delta, or `None` when gs is absent (already skipped).
fn assert_matches_oracle(tag: &str, pdf: &[u8], tolerance: f64) -> Option<f64> {
    let out = export(Arc::new(pdf.to_vec()), &[0], PsFormat::Ps).expect("the fixture exports");
    let gs = gs_raster(&out.bytes, tag)?;
    let mine = pdfcraft_raster(pdf);
    assert_eq!((gs.0, gs.1), (mine.0, mine.1), "the two rasters of {tag} cover the same canvas");
    let delta = mean_channel_delta(&gs.2, &mine.2);
    eprintln!("{tag}: mean per-channel delta {delta:.3}/255 (tolerance {tolerance})");
    if let Ok(dir) = std::env::var("PDFCRAFT_ORACLE_DUMP") {
        dump_png(std::path::Path::new(&dir).join(format!("{tag}-gs.png")), gs.0, gs.1, &gs.2);
        dump_png(std::path::Path::new(&dir).join(format!("{tag}-mine.png")), mine.0, mine.1, &mine.2);
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{tag}.pdf")), pdf);
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{tag}.ps")), &out.bytes);
    }
    assert!(delta < tolerance, "{tag}: gs and PdfCraft rasters diverge (mean per-channel delta {delta:.3} ≥ {tolerance})");
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
/// engine). `None` (after a skip message) when gs is not installed — unless
/// `PDFCRAFT_ORACLES_REQUIRED` is set, when the missing oracle fails the test
/// (crates/oracle's contract). `-q` keeps
/// gs's banner out of the capture, so the string is exactly what txtwrite
/// wrote (CRLF line endings included — assert with `contains`, not equality).
/// The Type 1 subsetting gate and the Inter extraction gate reuse this.
fn gs_text(ps: &[u8], tag: &str) -> Option<String> {
    let base = std::env::temp_dir().join(format!("pdfcraft-ps-oracle-{}-{tag}-txt", std::process::id()));
    let ps_path = base.with_extension("ps");
    std::fs::write(&ps_path, ps).expect("writing the oracle's temp .ps cannot fail");
    let run = Command::new("gs").args(["-q", "-dSAFER", "-dBATCH", "-dNOPAUSE", "-sDEVICE=txtwrite", "-sOutputFile=-"]).arg(&ps_path).output();
    let output = match run {
        Ok(output) => output,
        Err(why) => {
            let _ = std::fs::remove_file(&ps_path);
            assert!(
                std::env::var_os("PDFCRAFT_ORACLES_REQUIRED").is_none(),
                "gs is required (PDFCRAFT_ORACLES_REQUIRED is set) but could not be spawned: {why}"
            );
            eprintln!("skipping the {tag} text oracle: gs could not be spawned ({why})");
            return None;
        }
    };
    assert!(output.status.success(), "gs rejected the exported {tag} program for text extraction:\n{}", String::from_utf8_lossy(&output.stderr));
    let _ = std::fs::remove_file(&ps_path);
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Geometry: fills (both rules), strokes, dashes, Bézier fills and round joins — the same
/// image from both rasterizers. Observed delta ≈ 0.7 against PdfCraft's own raster (edge
/// coverage quantization). A separate manual gs-vs-gs run (gs on the exported program
/// against gs on the source PDF, matched policy) measured 0.000 — a hand-run observation
/// this test does not make: it compares gs's raster with PdfCraft's, never gs with gs.
#[test]
fn geometry_page_matches_ghostscript() {
    assert_matches_oracle("geometry", &geometry_pdf(), TOLERANCE);
}

/// Flat DeviceCMYK paints leave as `setcmykcolor` with the raw components (`0 1 1 0 k`
/// becomes `0 1 1 0 setcmykcolor`), beside a raw-`setgray` grey fill and an RGB fill, a
/// CMYK stroke and mixed-component fills — the same image from both rasterizers, under the
/// CMYK-specific tolerance (the two CMYK→RGB pipelines differ; see [`CMYK_TOLERANCE`]).
/// Observed delta ≈ 0.5.
#[test]
fn cmyk_page_matches_ghostscript() {
    assert_matches_oracle("cmyk", &cmyk_pdf(), CMYK_TOLERANCE);
}

/// `/Separation` spot colours leave as prolog colorspace resources (`/PdfCraftCS<n>`,
/// tint transforms as procedures) selected by `setcolorspace` + `setcolor` — gs applies
/// each tint transform through the space's alternate device space exactly as PdfCraft's
/// raster applies it (range clamps included, which the calculator's out-of-range outputs
/// exercise), so the same image comes out of both rasterizers, under the CMYK-specific
/// tolerance (the CMYK-alternate regions convert through the same two differing
/// pipelines as the CMYK fixture; see [`CMYK_TOLERANCE`]). Observed delta ≈ 0.5, the
/// CMYK fixture's.
#[test]
fn separation_page_matches_ghostscript() {
    assert_matches_oracle("separation", &separation_pdf(), CMYK_TOLERANCE);
}

/// The DCTDecode image at a 1:1 pixel mapping: same JPEG bytes, two IDCTs. Observed
/// delta ≈ 0.003 (JPEG decoder rounding on a handful of samples).
#[test]
fn image_page_matches_ghostscript() {
    assert_matches_oracle("image", &image_pdf(), TOLERANCE);
}

/// Text over the Layer A/B strategies: Type 1 as `show` (gs's FreeType against skrifa's
/// outlines of the same box charstrings), embedded TrueType as Type 42 `show` (gs resolving
/// the assigned codes through the font's `/Encoding` names and `/CharStrings`), and a
/// non-embedded font as vectorized outlines. Observed delta ≈ 0.2.
#[test]
fn text_page_matches_ghostscript() {
    assert_matches_oracle("text", &text_pdf(), TOLERANCE);
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
    assert_matches_oracle("t1-subset", &pdf, TOLERANCE);
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
    assert_matches_oracle("real-truetype", &pdf, TOLERANCE);
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

/// The W1 AGL naming gate (Task 5): a post-3.0 TrueType — no glyph names at
/// all — leaves as a Type 42 whose `/Encoding` and `/CharStrings` carry
/// `uniXXXX` names synthesized from the program's own cmap (the reverse
/// map), and those names make the text extractable: gs's txtwrite resolves
/// the `uniXXXX` spelling through the Adobe Glyph List, where the `/gNNNN`
/// fallback is meaningless — the honest degradation before this task. Ink on
/// the oracle side proves the drawn code still resolves
/// Encoding → name → CharStrings → gid. Raster parity for this fixture is
/// [`text_page_matches_ghostscript`], whose F3 is the same font drawn as
/// "AB". Skips its gs halves when gs is not installed.
#[test]
fn agl_post3_names_extract() {
    let pdf = assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        // F1: the post-3.0 synthetic TrueType (descriptor 5, program 6), 'A' alone drawn.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /PdfCraftTT /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 5 0 R >>".to_vec(),
        stream_obj("", b"BT /F1 72 Tf 14.4 100 Td (A) Tj ET"),
        b"<< /Type /FontDescriptor /FontName /PdfCraftTT /Flags 4 /FontBBox [0 -200 700 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>".to_vec(),
        stream_obj("", &ttf_program()),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
    ]);
    let out = export(Arc::new(pdf), &[0], PsFormat::Ps).expect("the post-3.0 page exports");
    assert!(out.warnings.is_empty(), "the synthesized names degrade nothing: {:?}", out.warnings);
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("/FontType 42"), "the Type 42 dictionary emits: {s}");
    assert!(s.contains("/uni0041"), "the mapped glyph carries its codepoint as its name: {s}");
    assert!(!s.contains("/g0001"), "the mapped glyph no longer rides the meaningless fallback: {s}");
    let Some(text) = gs_text(&out.bytes, "agl-post3") else {
        return;
    };
    assert!(text.contains('A'), "the name-less glyph's text is extractable through its AGL name: {text:?}");
    let Some((_, _, rgb)) = gs_raster(&out.bytes, "agl-post3") else {
        return;
    };
    // Measured ≈ 300 inked pixels (the filled 300×400-unit box at 72 pt =
    // 15×20 px at the oracle's 50 dpi); the threshold keeps 2× headroom below
    // that while sitting far above the blank a broken name→gid chain paints.
    let inked = rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 200).count();
    assert!(inked > 150, "the drawn glyph painted {inked} inked pixels through its synthesized name");
}

/// Axial and radial shadings: gs's `shfill` interpolation against the raster renderer's
/// gradients. Observed delta ≈ 0.1 (gradient interpolation differs by design).
#[test]
fn shading_page_matches_ghostscript() {
    assert_matches_oracle("shading", &shading_pdf(), TOLERANCE);
}

/// EPS output of the same geometry page rasterizes identically (gs runs the file and a
/// trailing `showpage` ejects the page the importing application would own). Observed
/// delta ≈ 0.7, the geometry fixture's.
#[test]
fn eps_output_raster_matches_ghostscript() {
    let pdf = geometry_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Eps).expect("the fixture exports as EPS");
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
            let _ = std::fs::remove_file(&eps_path);
            assert!(
                std::env::var_os("PDFCRAFT_ORACLES_REQUIRED").is_none(),
                "gs is required (PDFCRAFT_ORACLES_REQUIRED is set) but could not be spawned: {why}"
            );
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

/// A page of tiling patterns: one blue square per 14.4 pt cell across the whole page box —
/// the Task 4 `makepattern`/`setpattern` emission surface. The pattern's dict and use-site
/// shapes are measured against gs, not assumed: the resource name is *evaluated* and the
/// dictionary is the first `makepattern` operand (gs 10.08.0 typechecks `/Name makepattern`
/// and `matrix dict makepattern`). The geometry sits on the oracle's 1.44 pt pixel grid, so
/// both rasterizers step the tiles pixel-aligned.
fn tiling_pdf() -> Vec<u8> {
    assemble(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 288 360] >>".to_vec(),
        stream_obj("", b"/Pattern cs /P1 scn 0 0 288 360 re f"),
        b"<< /Type /Page /Parent 2 0 R /Contents 3 0 R /Resources << /Pattern << /P1 5 0 R >> >> >>".to_vec(),
        stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 14.4 14.4] /XStep 14.4 /YStep 14.4 /Resources << >>",
            b"0 0 1 rg 2.88 2.88 8.64 8.64 re f",
        ),
    ])
}

/// Tiling patterns: gs must accept the exported pattern resource (its PaintProc, the
/// `makepattern`/`setpattern` use site) and step the tiles where PdfCraft's own tiling of
/// the same source steps them. Repeated tiles must be visible in gs's raster — ink present
/// but well under half the page (the blue square covers 36% of each cell), which a flat
/// wash or a blank pattern would each violate. Skips its gs half when gs is not installed.
#[test]
fn tiling_page_matches_ghostscript() {
    let out = export(Arc::new(tiling_pdf()), &[0], PsFormat::Ps).expect("the tiling page exports");
    assert!(out.warnings.is_empty(), "a tiling fill degrades nothing: {:?}", out.warnings);
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("makepattern setpattern\n"), "the pattern use site: {s}");
    let Some((_, _, rgb)) = gs_raster(&out.bytes, "tiling") else {
        return;
    };
    let inked = rgb.as_chunks::<3>().0.iter().filter(|c| c[2] > 150 && c[0] < 100).count();
    // Measured ≈ 18000 inked pixels (36% of the 200×250 canvas); the bounds keep headroom
    // either side while separating a repeating pattern from a wash or a blank.
    assert!(inked > 5000 && inked < (PX.0 * PX.1 / 2) as usize, "the tiles repeat in gs's raster: {inked} inked pixels");
    assert_matches_oracle("tiling", &tiling_pdf(), TOLERANCE);
}

/// Transparency flattens (Wave 2): a real-transparency group's vector ops are
/// withheld and replaced by exactly one opaque raster, the flattening is disclosed
/// in the notes and in the file, and the program stays balanced. The raster half of
/// the gate is [`transparency_page_matches_ghostscript`].
#[test]
fn transparency_page_exports_with_note() {
    let out = export(Arc::new(transparency_pdf()), &[0], PsFormat::Ps).expect("transparent content exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("0 setgray\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster replaces the group: {s}");
    assert!(!s.contains("0 0 1 setrgbcolor"), "the group's own vector bytes were withheld and discarded: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
}

/// The flattened `/ca 0.5` group page against gs: the export is backdrop vectors
/// plus one 300-dpi region raster, gs resamples that raster onto its 50-dpi grid,
/// and PdfCraft renders the source's half-opacity stacked fills directly — the mean
/// per-channel delta must sit under [`FLATTEN_TOLERANCE`] (see its doc for the
/// resampling asymmetry). The stacked-alpha overlap of the group's two fills and
/// the black backdrop beneath the region are what the comparison composites through.
/// Observed delta 0.000 (grid-aligned flat regions; see [`FLATTEN_TOLERANCE`]).
#[test]
fn transparency_page_matches_ghostscript() {
    assert_matches_oracle("transparency", &transparency_pdf(), FLATTEN_TOLERANCE);
}

/// The `/SMask` alpha-mask page: byte level, one raster carries the masked fill —
/// the mask form's own black rect never leaks as vector ops, the backdrop's vectors
/// survive, the disclosure travels, the program stays balanced — then the raster
/// comparison under [`FLATTEN_TOLERANCE`]: gs's rendering of the masked source
/// (its own soft-mask machinery) against the flattened region, whose replay drew
/// the mask through hayro's `draw_soft_mask`. A dropped mask — yesterday's
/// note-only behaviour — paints the blue across the whole rect and fails by tens
/// of levels. Observed delta 0.000 (the hard mask edge sits on the pixel grid;
/// see [`FLATTEN_TOLERANCE`], whose doc records the mis-placed-mask bug this
/// gate caught at ≈ 44.9).
///
/// The page also carries `transparency ignored (soft mask)` beside the flattening
/// disclosure: the interpreter restores the graphics state after the form and
/// re-announces the mask it still holds, and the note fires at announcement time
/// (Task 10's once-per-page policy) even though no draw ever used it outside the
/// group — the raster carried it. The gate asserts the flattening note (what the
/// export did), not the absence of the announcement note.
#[test]
fn soft_mask_page_matches_ghostscript() {
    let pdf = soft_mask_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the soft-mask page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster carries the masked draw: {s}");
    assert!(!s.contains("0 0 1 setrgbcolor"), "the masked fill's vector bytes were withheld and discarded: {s}");
    assert!(!s.contains("0 setgray"), "the mask form's black rect never leaks as vector ops: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("soft-mask", &pdf, FLATTEN_TOLERANCE);
}

/// The `/BM /Multiply` group page: byte level, one raster carries the blended draw —
/// the backdrop's vector ops survive, the group's own blue fill never leaks as
/// vector ops, the disclosure travels, no blend note fires (the raster carries the
/// blend; the edge rule's in-group side), and the program stays balanced — then the
/// raster comparison under [`FLATTEN_TOLERANCE`]: gs resamples the flattened raster
/// (whose replay composited Multiply inside the scene, through vello) onto its
/// 50-dpi grid while `pdfcraft_raster` composites the source's blend directly. A
/// dropped blend paints the blue over the yellow band and fails by tens of levels
/// (the non-vacuity probe, forcing the replay's group layer to `Normal`, measured
/// 21.420). Observed delta 0.000 (grid-aligned flat regions; see
/// [`FLATTEN_TOLERANCE`]).
#[test]
fn blend_mode_page_matches_ghostscript() {
    let pdf = blend_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the blend page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster carries the blended draw: {s}");
    assert!(!s.contains("0 0 1 setrgbcolor"), "the group's blue fill never leaks as vector ops: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(out.warnings.iter().all(|n| !n.text.contains("blend")), "no blend note inside the flattened region: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("blend", &pdf, FLATTEN_TOLERANCE);
}

/// Nested real-transparency groups (W3): byte level, the final stream carries
/// **exactly one** raster — the inner group's pop flattens its own region and its
/// raster lands in the *outer* group's withheld buffer, which the outer's pop
/// discards on success, so the outer's union raster is the only survivor — placed
/// over the **union** region (`extend` widens every open frame, so the outer's
/// frame covers the inner group's protruding red fill): the placement concat and
/// the raster's pixel dimensions pin it (x 42.22–274.58, y 42.22–332.18 pt — the
/// union of both fills' reach, grown by the flattener's slack). The backdrop's
/// vector ops survive, neither group's colour ops leak, the flattening is
/// disclosed once, nothing declined (no `transparency ignored` note), and the
/// program stays balanced. Then the raster comparison under
/// [`FLATTEN_TOLERANCE`]: gs resamples the flattened union (whose replay
/// composited the 0.5-opacity blue fill and the nested 0.9-opacity red fill
/// through vello's stacked group layers) onto its 50-dpi grid while
/// `pdfcraft_raster` composites the source's nested groups directly. Observed
/// delta 0.000 (grid-aligned flat regions; see [`FLATTEN_TOLERANCE`]).
///
/// Non-vacuity, both halves measured: a probe widening only the innermost frame
/// (`extend` touching just `frames.last_mut()`) crops the outer's region to the
/// blue box — the placement and width assertions fail and the raster comparison
/// measures **12.043** (the protruding red missing over the white page); and a
/// probe spending the flatten budget on the inner group alone (the outer forced
/// to decline) flushes the withheld bytes back — the blue fill *and the inner
/// group's raster* leak into the program, and the leak assertions fail, proving
/// the inner raster really was produced into the withheld buffer.
#[test]
fn nested_groups_page_matches_ghostscript() {
    let pdf = nested_groups_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the nested-group page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("0 setgray\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster survives - the outer pop discarded the inner's: {s}");
    assert!(!s.contains("0 0 1 setrgbcolor"), "the outer group's blue fill never leaks as vector ops: {s}");
    assert!(!s.contains("1 0 0 setrgbcolor"), "the inner group's red fill never leaks as vector ops: {s}");
    // The union coverage: the raster is placed over the union of both fills' reach
    // (each grown by the flattener's 0.5-unit + 2-px slack), and its pixel grid is
    // the snapped union - 970x1210 px at 300 dpi over x 42.22-274.58, y 42.22-332.18 pt.
    assert!(s.contains("[0.24 0 0 -0.24 42.22 332.18] concat\n"), "the raster is placed over the union region: {s}");
    assert!(s.contains("/Width 970 /Height 1210"), "the raster covers the union's snapped pixel grid: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(out.warnings.iter().all(|n| !n.text.starts_with("transparency ignored")), "nothing declined: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("nested-groups", &pdf, FLATTEN_TOLERANCE);
}

/// A tiling pattern inside an opacity group, with the Task 12 deferred combination
/// on the same page (entry-state `/ca 0.5` + in-form `/BM /Multiply`): byte level,
/// one raster carries the whole group — the red under-fill, the Multiply and the
/// tiles alike; the `makepattern setpattern` use site never leaks (those bytes were
/// withheld with the draw and discarded), the yellow backdrop's vector ops survive,
/// no blend note fires (the Multiply was announced inside the open real group —
/// the edge rule's in-group side; the raster carries it), and the program stays
/// balanced. The pattern resource itself still rides the prolog unreferenced:
/// interned at draw time into the emitter's table, which the withhold cannot
/// unwind — harmless legal PostScript (the font prolog does the same for the
/// glyph gate below), disclosed here so nobody mistakes it for a leak. Then the
/// raster comparison under [`FLATTEN_TOLERANCE`]: the flattened raster's replay
/// multiplied the blue tiles against the red under-fill inside the isolated group
/// (black squares) and composited the group at half opacity over the yellow
/// backdrop, exactly what `pdfcraft_raster` composites. Observed delta 0.000 —
/// the tiles step on the oracle's own grid (36 black of every 60 raster pixels;
/// see [`FLATTEN_TOLERANCE`]).
///
/// **This gate caught a real bug on its first run, at a measured 11.870**: the
/// flattener recorded the pattern paint verbatim, but the renderer's tiling
/// algebra takes the pattern's matrix (and its cell pixel density) to live in the
/// *device* space of the draws beside it — under `page_render` the interpreter's
/// root transform carries the render scale, while this crate's export drive runs
/// in page points, so the replay tiled its cell at one pixel per *point*: the
/// wrong size, phase and sharpness (a moiré of 14-px tiles where 60-px ones
/// belong). The fix re-roots the pattern onto the scene's pixel space at record
/// time (`Pattern::re_rooted`, the `SoftMask::re_rooted` counterpart;
/// `flatten::re_rooted_paint`). A dropped-tile probe (skipping the pattern
/// draw's scene recording) fails at a measured **9.339** — the red under-fill
/// shows where the black squares belong.
#[test]
fn tiling_inside_group_matches_ghostscript() {
    let pdf = tiling_inside_group_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the tiling-in-group page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster carries the group's draws: {s}");
    assert!(!s.contains("makepattern"), "the tiling use site was withheld and discarded: {s}");
    assert!(!s.contains("1 0 0 setrgbcolor"), "the red under-fill never leaks as vector ops: {s}");
    assert!(s.contains("%%BeginResource: pattern "), "the pattern resource may still ride the prolog (harmless interning): {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(out.warnings.iter().all(|n| !n.text.contains("blend")), "no blend note inside the flattened region: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("tiling-group", &pdf, FLATTEN_TOLERANCE);
}

/// A JPEG image and an embedded-TrueType text run inside one opacity group: byte
/// level, one raster carries both — the text run stays pending across the image
/// draw and flushes (at the form's BBox clip pop, still inside the group) into the
/// **withheld** buffer, where the raster replaces it, so neither the flushed
/// `xyshow` nor the JPEG's DCT passthrough invocation leaks into the final
/// program. The black backdrop's vector ops survive, the disclosure travels, the
/// program stays balanced, and the raster's placement pins the union (the glyph
/// boxes' reach joins the image's: the region starts at x 13.884 pt, left of the
/// image's 27.6, and its 668-px width covers out past the image's right edge).
/// The Type 42 font dictionary still rides the prolog unreferenced — the same
/// harmless interning as the tiling gate's pattern resource. Then the raster
/// comparison under [`FLATTEN_TOLERANCE`]: the raster must carry the JPEG
/// (decoded by the same hayro machinery both sides use) *under* the green glyph
/// boxes (draw order), at the same half-opacity the source's group applies.
/// Observed delta 0.000 (grid-aligned flat regions and an exact 6:1 image
/// resample; see [`FLATTEN_TOLERANCE`]).
///
/// Non-vacuity, both halves: a probe forcing the run's flush outside the withheld
/// buffer (deferring it past the parent restore) leaks the `xyshow` and the DCT
/// invocation into the program and the byte assertions fail; a probe skipping the
/// glyph's scene recording drops the boxes from the raster's region — the
/// placement/width assertions fail while the raster comparison measures only
/// **0.446** (the glyph boxes are small), which is exactly why the byte-level
/// union assertion is the gate's discriminator, not the delta alone.
#[test]
fn image_and_glyph_inside_group_matches_ghostscript() {
    let pdf = image_and_glyph_inside_group_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the image+glyph page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("0 setgray\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster carries the group's draws: {s}");
    assert!(!s.contains("xyshow"), "the flushed text run never leaks outside the withheld buffer: {s}");
    assert!(!s.contains("/DCTDecode filter"), "the JPEG's own image invocation never leaks: {s}");
    assert!(s.contains("/FontType 42"), "the Type 42 dictionary may still ride the prolog (harmless interning): {s}");
    assert!(s.contains("[0.24 0 0 -0.24 13.884 253.2] concat\n"), "the raster is placed over the image-glyph union: {s}");
    assert!(s.contains("/Width 668 /Height 760"), "the raster covers the union's snapped pixel grid: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("img-glyph-group", &pdf, FLATTEN_TOLERANCE);
}

/// A shading pattern inside an opacity group (W3 review gate): byte level, one
/// raster carries the gradient — the `shfill` route's operators were withheld
/// with the draw and discarded, the yellow backdrop's vector ops survive, the
/// disclosure travels, and the program stays balanced — then the raster
/// comparison under [`FLATTEN_TOLERANCE`]: the flattened raster replays the
/// shading through the re-rooted pattern matrix (300-dpi density) with its
/// `/BBox` clip rebased through the page matrix **alone**, so the gradient
/// spans x 28.8–172.8 and the clip's right edge and top strip show the yellow
/// backdrop exactly as `pdfcraft_raster` composites the source. The
/// pre-fix double-transformed clip (the captured matrix applied twice) moved
/// the clip box to x 86.4–374.4, y 129.6–647: the gradient's left band
/// vanished and its extended blue tail painted past the true edge — the unit
/// gate `device.rs::shading_pattern_clip_flatten_keeps_the_bbox_placed` pins
/// those pixels; this gate is the end-to-end gs proof. Observed delta **0.024**
/// (a smooth gradient resampled 6:1 nearest against a direct 50-dpi evaluation
/// differs by the gradient's slope over a sub-pixel — the flat fixtures'
/// byte-identity does not apply here; see [`FLATTEN_TOLERANCE`], whose doc also
/// records the pre-fix failure this gate measured at ≈ 37.2).
#[test]
fn shading_inside_group_matches_ghostscript() {
    let pdf = shading_inside_group_pdf();
    let out = export(Arc::new(pdf.clone()), &[0], PsFormat::Ps).expect("the shading-in-group page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "exactly one raster carries the shaded draw: {s}");
    assert!(!s.contains("shfill"), "the shading route's operators were withheld and discarded: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
    assert_matches_oracle("shading-group", &pdf, FLATTEN_TOLERANCE);
}

/// A huge but finite group box (Task 14's hostile-geometry gate): the group's
/// only draw sits at ~1e7 pt, entirely off the page, so the frame's box is
/// enormous yet finite — no cap misfires, no panic, and the region ∩ page is
/// empty. What that empty region does is Task 8's reviewed contract, pinned
/// here end-to-end: the flattener returns an empty raster (not a decline), the
/// withheld vector bytes are discarded — they painted nothing on the page, and
/// the bbox over-approximates the draw's reach, so nothing visible is lost —
/// and the flattening still discloses itself. The task brief predicted a
/// decline with today's "transparency ignored (transparency group)" note; the
/// implementation's empty-region arm is the deliberate behaviour (the unit
/// side is `flatten.rs::empty_region_yields_empty_raster`), so this gate pins
/// it as it is: completes, balanced, zero rasters, no group ink leaked, the
/// flattening note, no decline note.
#[test]
fn huge_group_bbox_flattens_to_an_empty_region() {
    let out = export(Arc::new(huge_group_pdf()), &[0], PsFormat::Ps).expect("the huge-bbox group page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 0, "an empty region emits no raster: {s}");
    assert!(!s.contains("0 0 1 setrgbcolor"), "the off-page draw's vector bytes were withheld and discarded: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the empty flattening is disclosed: {:?}", out.warnings);
    assert!(
        out.warnings.iter().all(|n| n.text != "transparency ignored (transparency group)"),
        "an empty region is not a decline: {:?}",
        out.warnings
    );
    assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
}

/// The per-page flatten budget, end-to-end (Task 14): 129 real groups on one
/// page spend the budget of 128 — the first 128 pops each flatten (their
/// vector bytes replaced by one opaque raster each) and the 129th declines,
/// flushing its withheld vector back into the program with today's note.
/// Never silent, never lost: exactly 128 rasters, exactly one surviving group
/// fill (the 129th's), both disclosure notes exactly once each (the
/// once-per-page policy), and a balanced program. The scene-level side
/// (`flatten.rs::flattens_cap_declines`) proves the budget check itself
/// through real recorded group entries.
#[test]
fn flattens_cap_declines() {
    let out = export(Arc::new(groups_budget_pdf()), &[0], PsFormat::Ps).expect("the 129-group page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert_eq!(s.matches(">> image").count(), 128, "the first 128 groups each leave one raster: {s}");
    assert_eq!(s.matches("0 0 1 setrgbcolor").count(), 1, "the 129th group's vector fill flushes back, and only it: {s}");
    assert_eq!(
        out.warnings.iter().filter(|n| n.text == "transparency flattened to raster").count(),
        1,
        "the flattening is disclosed once per page: {:?}",
        out.warnings
    );
    assert_eq!(
        out.warnings.iter().filter(|n| n.text == "transparency ignored (transparency group)").count(),
        1,
        "the budget decline is disclosed once per page: {:?}",
        out.warnings
    );
    assert!(s.contains("%%PdfCraftNote: transparency ignored (transparency group)\n"), "the decline note embedded: {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
}

/// The program with raster payloads stripped: an ASCII85 stream can contain
/// any printable triple by chance (a `nan`/`inf` inside image data is bytes,
/// not an operand), so token scans over operators must not read it. Payloads
/// run from each `>> image`/`>> imagemask` line to the `~>` terminator (the
/// `images.rs` dictionary-form emission).
fn without_image_payloads(s: &str) -> String {
    let mut out = String::new();
    let mut in_payload = false;
    for line in s.lines() {
        if in_payload {
            in_payload = !line.trim_end().ends_with("~>");
            continue;
        }
        in_payload = line.contains(">> image");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Non-finite geometry inside a group (Task 14): the form's `/BBox` carries an
/// f32-overflowing entry, so the interpreter pushes the form's clip with an
/// infinite x1 — the one route non-finite coordinates reach the emitter
/// through `export()` (overflowing content operands are dropped by the
/// interpreter before any draw; observed in this task's probes). The clip's
/// path coordinates are repaired to `0` with the fidelity note, the group's
/// own draw is an ordinary on-page fill, and the group flattens: the replay
/// pushes the infinite clip through vello under the panic guard and produces
/// the raster. No panic, no non-finite operand in the program, checked math
/// intact, export completes balanced — the never-crash contract under
/// deep-hostile input. (The repair-note count is the interpreter's clip-push
/// count — observed 4, a vendor internal — so the gate pins presence, not the
/// exact figure.)
#[test]
fn non_finite_group_geometry() {
    let out = export(Arc::new(non_finite_group_pdf()), &[0], PsFormat::Ps).expect("the non-finite-geometry page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(
        out.warnings.iter().filter(|n| n.text == "non-finite coordinate replaced with 0").count() >= 1,
        "the infinite clip coordinate is repaired with a note: {:?}",
        out.warnings
    );
    let lowered = without_image_payloads(&s).to_lowercase();
    assert!(!lowered.contains("nan") && !lowered.contains("inf"), "no non-finite operand in the program: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "the finite draw inside the hostile clip still flattens: {s}");
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    assert!(out.warnings.iter().all(|n| n.text != "transparency ignored (transparency group)"), "nothing declined: {:?}", out.warnings);
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
}

/// The nested-budget interaction (Task 14): a tiling cell that names itself,
/// painted inside a real-transparency group. The vendor's paint budget stops
/// the recursion — the deepest `interpret` returns `None` ("cell undecodable")
/// and the levels above it collect cells that fill through the nested pattern,
/// so a chain of pattern resources rides the prolog unreferenced (the same
/// harmless interning the tiling-in-group gate discloses). The group's pop
/// then flattens: one raster carries the region — the replay's renderer meets
/// the same budget refusal the reference renderer meets, so the composite is
/// faithful by construction — and the flat-gray stand-in drawn inside the
/// withheld buffer is discarded with it, while the fidelity notes (recorded
/// globally, never in a withheld buffer) disclose both the cell decline and
/// the flattening. The decline path composes: a nested refusal never sinks
/// the group, never panics, never loses output, and the program stays
/// balanced with no pattern use site leaking into the page body.
#[test]
fn self_referencing_tiling_cell_inside_group_composes() {
    let out = export(Arc::new(self_referencing_cell_group_pdf()), &[0], PsFormat::Ps).expect("the self-referencing-cell page exports");
    let s = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(s.contains("1 1 0 setrgbcolor\n"), "the backdrop's vector ops survive: {s}");
    assert_eq!(s.matches(">> image").count(), 1, "the group flattens - the nested refusal does not sink it: {s}");
    assert_eq!(
        out.warnings.iter().filter(|n| n.text == "tiling pattern approximated (cell undecodable)").count(),
        1,
        "the budget-bound decline is disclosed once per page: {:?}",
        out.warnings
    );
    assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "the flattening is disclosed: {:?}", out.warnings);
    let (_, page_body) = s.split_once("%%Page: 1 1").expect("the program has a page body");
    assert!(!page_body.contains("makepattern"), "no pattern use site leaks into the page body: {s}");
    assert!(s.contains("%%BeginResource: pattern "), "the collected cell chain may ride the prolog (harmless interning): {s}");
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balanced: {s}");
}

/// A degraded shading paints ink: the oversized-mesh fixture degrades to the flat
/// mid-tone, and the exported fallback's `fill` must cover the page in gs's raster (the
/// pre-fix `clip`-then-`fill` fallback painted nothing — gs-verified: 0 inked pixels).
/// The bytes are checked unconditionally (a `fill` that follows a rebuilt `newpath…`
/// sequence), the raster wherever gs is installed.
#[test]
fn degraded_shading_paints_ink() {
    let out = export(Arc::new(degraded_shading_pdf()), &[0], PsFormat::Ps).expect("the fixture exports");
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
fn cff_promotion_page_matches_ghostscript() {
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
    assert_matches_oracle("cff-promote", &pdf, TOLERANCE);
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
/// [`cff_promotion_page_matches_ghostscript`] for the probes), so
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
    assert_matches_oracle("cid-cjk", &pdf, TOLERANCE);
}
