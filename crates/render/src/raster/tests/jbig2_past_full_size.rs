//! A JBIG2 image past the bound on decoding an image at its full size (2^28 pixels) is drawn at a
//! reduced size, up to 2^29 pixels (vendored hayro-jbig2 patch 19, hayro-syntax patch 53,
//! hayro-interpret patch 113): a page that is one 1,200 dpi newspaper scan of 19,467 × 25,517
//! (PDFBOX-4598) drew blank. An image too large to draw at all is reported as content left out,
//! where it was skipped with nothing said.

use super::*;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect};

/// A 100 × 100 pt page drawing image XObject `/Im` (`width` × `height`, `entries`, stream
/// `data`) over the square 25..75, as `content` says, with objects from 6 up in `objects`.
fn image_page(width: u32, height: u32, entries: &str, data: &[u8], content: &str, objects: &[u8]) -> Vec<u8> {
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n",
        content.len()
    )
    .into_bytes();
    pdf.extend_from_slice(objects);
    pdf.extend_from_slice(
        format!("5 0 obj << /Type /XObject /Subtype /Image /Width {width} /Height {height} {entries} /Length {} >> stream\n", data.len()).as_bytes(),
    );
    pdf.extend_from_slice(data);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    pdf
}

/// The page's pixel at `(x, y)`, top-down, rendered at 72 dpi.
fn pixel(page: &RenderedPage, x: usize, y: usize) -> [u8; 4] {
    let at = (y * page.width as usize + x) * 4;
    page.rgba[at..at + 4].try_into().unwrap()
}

const DRAW_IN_THE_MIDDLE: &str = "q 50 0 0 50 25 25 cm /Im Do Q";

/// A JBIG2 page just past 2^28 pixels, and one of 2^29, all black, is drawn black over the area it
/// is drawn at, as an image and as a stencil mask, with nothing reported. Before, the image was
/// skipped and the page came out white.
#[test]
fn jbig2_images_past_the_full_size_bound_are_drawn_reduced() {
    for (width, height) in [(16385, 16384), (32768, 16384)] {
        let stream = black_jbig2_page_of(width, height);
        for entries in ["/BitsPerComponent 1 /ColorSpace /DeviceGray", "/ImageMask true"] {
            let pdf = image_page(width, height, &format!("{entries} /Filter /JBIG2Decode"), &stream, DRAW_IN_THE_MIDDLE, b"");
            let page = render_first_page(pdf);
            let what = format!("{width} × {height}, {entries}");
            assert!(page.warnings.is_empty(), "{what}: {:?}", page.warnings);
            assert_eq!(pixel(&page, 50, 50), [0, 0, 0, 255], "{what}: drawn black in the middle");
            assert_eq!(pixel(&page, 30, 30), [0, 0, 0, 255], "{what}: drawn black near its corner");
            assert_eq!(pixel(&page, 10, 10), [255, 255, 255, 255], "{what}: white outside it");
            assert_eq!(pixel(&page, 90, 50), [255, 255, 255, 255], "{what}: white beside it");
        }
    }
}

/// A JBIG2 soft mask past 2^28 pixels is decoded reduced too, so the picture it masks is drawn
/// through it. Before, the mask was refused, and the picture was left out with it.
#[test]
fn a_jbig2_soft_mask_past_the_full_size_bound_is_drawn_reduced() {
    let (width, height) = (16385, 16384);
    let mask = black_jbig2_page_of(width, height);
    // A 1 × 1 red picture under a mask whose black page, decoded the other way, is opaque.
    let mut objects = format!(
        "6 0 obj << /Type /XObject /Subtype /Image /Width {width} /Height {height} /BitsPerComponent 1 /ColorSpace /DeviceGray /Decode [1 0] /Filter /JBIG2Decode /Length {} >> stream\n",
        mask.len()
    )
    .into_bytes();
    objects.extend_from_slice(&mask);
    objects.extend_from_slice(b"\nendstream endobj\n");
    let pdf = image_page(1, 1, "/BitsPerComponent 8 /ColorSpace /DeviceRGB /SMask 6 0 R", &[255, 0, 0], DRAW_IN_THE_MIDDLE, &objects);
    let page = render_first_page(pdf);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!(pixel(&page, 50, 50), [255, 0, 0, 255], "the picture is drawn through its mask");
    assert_eq!(pixel(&page, 10, 10), [255, 255, 255, 255], "white outside it");
}

/// An image too large to decode is content left out at one of the renderer's limits, and the
/// page says so: an image past 2^28 pixels in any filter but JBIG2 (the 45,009 × 33,555 CCITT
/// floor plan of PDFBOX-3407 among them), a JBIG2 image past 2^29, an indexed JBIG2 image past
/// 2^28, whose palette needs its pixels' own values, a colour-keyed one past 2^28, whose key
/// ranges are in those values too, and one past 2^28 with a side past the JBIG2 decoder's 65,535,
/// which it refuses at any size, and one past 2^28 with a region past it, which the decoder refuses
/// for that region. Before, each was skipped as a failed image,
/// which the renderer doesn't report, and a page that was only that image came out white with
/// nothing said.
#[test]
fn images_too_large_to_decode_are_reported_as_content_left_out() {
    let fax = "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 45009 >>";
    let flate = "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /FlateDecode";
    let jbig2 = "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode";
    let indexed = "/BitsPerComponent 1 /ColorSpace [/Indexed /DeviceGray 1 <00FF>] /Filter /JBIG2Decode";
    let colour_keyed = "/BitsPerComponent 1 /ColorSpace /DeviceGray /Mask [0 0] /Filter /JBIG2Decode";
    let region_past_the_bitmap_bound = jbig2_page_with_a_region_past_the_bitmap_bound();
    let cases = [
        ("a CCITT floor plan", (45009, 33555), fax, vec![0; 16]),
        ("a Flate image past 2^28", (16385, 16384), flate, vec![0; 16]),
        ("a JBIG2 image past 2^29", (32768, 16385), jbig2, black_jbig2_page_of(32768, 16385)),
        ("an indexed JBIG2 image past 2^28", (16385, 16384), indexed, black_jbig2_page_of(16385, 16384)),
        ("a colour-keyed JBIG2 image past 2^28", (16385, 16384), colour_keyed, black_jbig2_page_of(16385, 16384)),
        ("a JBIG2 image past 2^28 with a side past 65,535", (65536, 4097), jbig2, black_jbig2_page_of(65536, 4097)),
        ("a JBIG2 image past 2^28 with a region the decoder refuses", (16385, 16384), jbig2, region_past_the_bitmap_bound),
    ];
    for (what, (width, height), entries, data) in cases {
        let content = format!("{DRAW_IN_THE_MIDDLE} 1 0 0 rg 0 0 10 10 re f");
        let page = render_first_page(image_page(width, height, entries, &data, &content, b""));
        assert_eq!(page.warnings, [RenderWarning::ContentTruncated], "{what}");
        assert_eq!(pixel(&page, 50, 50), [255, 255, 255, 255], "{what}: not drawn");
        assert_eq!(pixel(&page, 5, 95), [255, 0, 0, 255], "{what}: the rest of the page is drawn");
    }
}

/// A 16,385 × 16,384 JBIG2 page with an intermediate generic region as large as itself: past 2^28
/// pixels, which only the page bitmap of an image read reduced may be, so the decoder refuses it.
fn jbig2_page_with_a_region_past_the_bitmap_bound() -> Vec<u8> {
    let mut stream = black_jbig2_page_of(16385, 16384);
    jbig2_segment(&mut stream, 1, 36, &[], &jbig2_white_region(16385, 16384));
    stream
}

/// Counts what a page hands its device: raster images with their pixels and stencil masks with
/// theirs, each asked for at the size it is drawn at, as the renderer does.
#[derive(Default)]
struct Draws(usize);

impl<'a> Device<'a> for Draws {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        let [a, b, c, d, ..] = transform.as_coeffs();
        let target = ((a.hypot(b) * f64::from(image.width())).ceil() as u32, (c.hypot(d) * f64::from(image.height())).ceil() as u32);
        match image {
            Image::Raster(r) => r.with_rgba(|_, _| self.0 += 1, Some(target)),
            Image::Stencil(s) => s.with_stencil(|_, _| self.0 += 1, Some(target)),
        }
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// The reduced size depends on the image alone, not on the size it is drawn at, so a page drawing
/// the image at three sizes decodes it once (its decoding takes about a second for a real page of
/// this kind): each zoom and each tile of the viewer is drawn from the one decode kept.
#[test]
fn a_reduced_jbig2_image_is_decoded_once_for_every_size_it_is_drawn_at() {
    let (width, height) = (16385, 16384);
    let content = "q 50 0 0 50 25 25 cm /Im Do Q q 20 0 0 20 0 0 cm /Im Do Q q 90 0 0 90 5 5 cm /Im Do Q";
    let pdf = image_page(width, height, "/ImageMask true /Filter /JBIG2Decode", &black_jbig2_page_of(width, height), content, b"");
    let pdf = Pdf::new(Arc::new(pdf)).expect("parses");
    let page = &pdf.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx =
        Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 100.0, 100.0), &cache, page.xref(), InterpreterSettings::default());
    let mut device = Draws::default();
    interpret_page(page, &mut ctx, &mut device);
    assert_eq!(device.0, 3, "drawn three times");
    assert_eq!(cache.decoded_image_decodes(), 1, "decoded once");
}

/// Bits, most significant first, as T.4 and T.6 codes are written.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, code: &str) {
        for bit in code.bytes() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if bit == b'1' {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.used % 8);
            }
            self.used += 1;
        }
    }
}

/// A JBIG2 page of 4,100 × 65,535 (268,693,500 pixels, just past 2^28), white on its left 2,048
/// columns and black on the rest: one immediate generic region over the page, MMR coded (ITU-T
/// T.88 6.2.6, which codes it as T.6 does). Its first row is a horizontal mode (`001`), a white run
/// of 2,048 (extended make-up code 2048, then terminating code 0) and a black run of 2,052 (make-up
/// 2048, terminating 4); every row after it repeats the one above, two vertical modes of 0 (`1`).
fn half_black_mmr_page() -> (u32, u32, Vec<u8>) {
    let (width, height) = (4100_u32, 65535_u32);
    let mut mmr = Bits::default();
    mmr.put("001");
    mmr.put("000000010011");
    mmr.put("00110101");
    mmr.put("000000010011");
    mmr.put("011");
    for _ in 1..height {
        mmr.put("11");
    }
    let mut page = Vec::new();
    page.extend_from_slice(&width.to_be_bytes());
    page.extend_from_slice(&height.to_be_bytes());
    page.extend_from_slice(&[0; 8]); // resolution
    page.extend_from_slice(&[0, 0, 0]); // flags (default pixel white), striping
    let mut region = Vec::new();
    region.extend_from_slice(&width.to_be_bytes());
    region.extend_from_slice(&height.to_be_bytes());
    region.extend_from_slice(&[0; 9]); // x, y, combination operator
    region.push(1); // MMR
    region.extend_from_slice(&mmr.bytes);
    let mut stream = Vec::new();
    jbig2_segment(&mut stream, 0, 48, &[], &page);
    jbig2_segment(&mut stream, 1, 38, &[], &region);
    (width, height, stream)
}

/// A JBIG2 image past 2^28 pixels is reduced by the smallest whole factor that keeps it within
/// 2^25 pixels (3 here, to 1,367 × 21,845), each sample the share of white among the pixels of its
/// block: white left of the edge between columns 2,047 and 2,048, black right of it, two thirds
/// white in the block across it, and black in the last column, which is two pixels wide. A caller
/// that doesn't take it reduced gets it refused, as before.
#[test]
fn a_reduced_jbig2_image_averages_each_block_of_pixels() {
    use hayro::hayro_syntax::object::stream::ImageDecodeParams;
    let (width, height, stream) = half_black_mmr_page();
    let pdf = image_page(width, height, "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode", &stream, DRAW_IN_THE_MIDDLE, b"");
    let pdf = Pdf::new(Arc::new(pdf)).expect("parses");
    let image = pdf.objects().into_iter().filter_map(|object| object.into_stream()).find(|stream| !stream.filters().is_empty()).unwrap();
    let params = ImageDecodeParams { width, height, reduce_past_full_size: true, ..ImageDecodeParams::default() };
    let decoded = image.decoded_image(&params).expect("decodes reduced");
    let data = decoded.image_data.unwrap();
    assert_eq!((data.width, data.height, data.bits_per_component), (1367, 21845, 8));
    assert_eq!(decoded.data.len(), 1367 * 21845);
    for row in [0, 1, 10_000, 21_844] {
        let samples = &decoded.data[row * 1367..][..1367];
        assert!(samples[..682].iter().all(|&s| s == 255), "row {row}: white left of the edge");
        assert_eq!(samples[682], 170, "row {row}: the block across the edge, two of its three columns white");
        assert!(samples[683..].iter().all(|&s| s == 0), "row {row}: black right of it, the narrow last column included");
    }
    assert!(image.decoded_image(&ImageDecodeParams { reduce_past_full_size: false, ..params }).is_err(), "refused at full size");

    // Drawn, it is the same: white on the left of its area, black on the right.
    let page = render_first_page(image_page(
        width,
        height,
        "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode",
        &stream,
        DRAW_IN_THE_MIDDLE,
        b"",
    ));
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!(pixel(&page, 30, 50), [255, 255, 255, 255], "white on its left");
    assert_eq!(pixel(&page, 70, 50), [0, 0, 0, 255], "black on its right");
}

/// The red channel along row 85 of a page drawing the image of `stream` (`width` × `height` in its
/// dictionary) under a `mask_width` × 2 soft mask of 32 bands, opaque and transparent in turn,
/// 90 × 10 pt along the bottom (rows 80 to 90): alone, and after a 1 pt draw near the top.
fn banded_mask_rows(width: u32, height: u32, stream: &[u8], mask_width: usize) -> (Vec<u8>, Vec<u8>) {
    let band = mask_width / 8 / 32;
    let row: Vec<u8> = (0..32).flat_map(|i| vec![if i % 2 == 0 { 0xff } else { 0x00 }; band]).collect();
    let mask = [row.clone(), row].concat();
    let mut objects = format!(
        "6 0 obj << /Type /XObject /Subtype /Image /Width {mask_width} /Height 2 /BitsPerComponent 1 /ColorSpace /DeviceGray /Length {} >> stream
",
        mask.len()
    )
    .into_bytes();
    objects.extend_from_slice(&mask);
    objects.extend_from_slice(
        b"
endstream endobj
",
    );
    let entries = "/BitsPerComponent 1 /ColorSpace /DeviceGray /SMask 6 0 R /Filter /JBIG2Decode";
    let large = "q 90 0 0 10 5 10 cm /Im Do Q";
    let alone = render_first_page(image_page(width, height, entries, stream, large, &objects));
    let after_small = render_first_page(image_page(width, height, entries, stream, &format!("q 1 0 0 1 5 90 cm /Im Do Q {large}"), &objects));
    assert!(after_small.warnings.is_empty(), "{:?}", after_small.warnings);
    let row = |page: &RenderedPage| (5..95).map(|x| pixel(page, x, 85)[0]).collect::<Vec<u8>>();
    let alone = row(&alone);
    let bands = alone.windows(2).filter(|w| w[0].abs_diff(w[1]) > 64).count();
    assert!(bands > 10, "drawn alone, the mask's bands show: {alone:?}");
    (alone, row(&after_small))
}

/// A reduced JBIG2 image whose soft mask is larger than the image as decoded (5,462 × 5,462 here)
/// but smaller than its full size has the mask averaged down to the size it is drawn at, so each
/// size it is drawn at gets its own alpha. The mask was compared with the full size, so the image
/// was cached once for every size, and the alpha averaged for a small draw was reused for a large
/// one: the mask's 256-pixel bands, averaged to an even grey for the small draw (16 samples
/// across), came out grey in the large one too.
#[test]
fn a_reduced_jbig2_image_keeps_a_soft_mask_for_each_size_it_is_drawn_at() {
    let (alone, after_small) = banded_mask_rows(16385, 16384, &black_jbig2_page_of(16385, 16384), 8192);
    assert_eq!(after_small, alone, "drawn after a small draw, it keeps its own mask");
}

/// The same when the JBIG2 page is another size than the dictionary says: 8,193 × 32,768 under a
/// dictionary of 16,385 × 16,384, so it decodes to 2,731 × 10,923, not the 5,462 × 5,462 the
/// dictionary predicts. Its 4,096-pixel-wide mask is larger than the first and smaller than the
/// second: it is kept whole, not averaged for one draw and reused for the next.
#[test]
fn a_reduced_jbig2_image_of_another_size_than_its_dictionary_keeps_its_soft_mask() {
    let (alone, after_small) = banded_mask_rows(16385, 16384, &black_jbig2_page_of(8193, 32768), 4096);
    assert_eq!(after_small, alone, "drawn after a small draw, it keeps its own mask");
}

/// A reduced JBIG2 image one of its own stream's limits refuses (a region past 2^28 pixels) is
/// refused again whenever it is decoded, so its failure is kept: drawn three times, it is decoded
/// once. Kept only for a failure the page's allowance caused, it was decoded at every draw, each
/// time up to the region it is refused at.
#[test]
fn a_reduced_jbig2_image_its_stream_refuses_is_decoded_once() {
    let (width, height) = (16385, 16384);
    let content = "q 50 0 0 50 25 25 cm /Im Do Q q 20 0 0 20 0 0 cm /Im Do Q q 90 0 0 90 5 5 cm /Im Do Q";
    let entries = "/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode";
    let pdf = image_page(width, height, entries, &jbig2_page_with_a_region_past_the_bitmap_bound(), content, b"");
    let pdf = Pdf::new(Arc::new(pdf)).expect("parses");
    let page = &pdf.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx =
        Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 100.0, 100.0), &cache, page.xref(), InterpreterSettings::default());
    let mut device = Draws::default();
    interpret_page(page, &mut ctx, &mut device);
    assert_eq!(device.0, 0, "not drawn");
    assert_eq!(cache.decoded_image_decodes(), 1, "decoded once");
}
