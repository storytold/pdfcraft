use std::sync::Arc;

use pdfcraft_testkit::Pdf;

use super::*;

const W: usize = 300;
const H: usize = 400;
/// Isolated specks on the paper, away from the text rows.
const SPECKS: [(usize, usize); 3] = [(150, 25), (200, 85), (120, 125)];

/// The light falling on the paper at `x`: full in the middle, darker towards a shadowed left
/// edge (as a book's gutter) and a little at the right edge.
fn light(x: usize) -> f32 {
    let left = (x as f32 / 60.0).min(1.0);
    let right = ((W - 1 - x) as f32 / 30.0).min(1.0);
    (0.55 + 0.45 * left) * (0.8 + 0.2 * right)
}

fn is_stroke(x: usize, y: usize) -> bool {
    (60..360).contains(&y) && (y - 60) % 40 < 6 && (60..240).contains(&x)
}

/// A synthetic scan, `n` components (1 grey, 3 RGB): greyish yellowed paper, a shadowed gutter,
/// dark text strokes and a few specks.
fn scan_pixels(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(W * H * n);
    for y in 0..H {
        for x in 0..W {
            let ink = if is_stroke(x, y) || SPECKS.contains(&(x, y)) { 0.12 } else { 0.8 };
            let v = ink * light(x);
            let px = [v, v, v * 0.85];
            for c in px.iter().take(n) {
                out.push((c * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

fn jpeg(px: &[u8], n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    image::ImageEncoder::write_image(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95), px, W as u32, H as u32, ty).unwrap();
    out
}

/// Three pages: 1 a scan (the image `image_dict`/`data` as object 4) with an invisible text
/// layer and a comment, 2 a page of real text, 3 the same scan image drawn again.
fn scan_pdf(image_dict: &str, data: &[u8]) -> Vec<u8> {
    let resources = "/Resources << /XObject << /Im0 4 0 R >> /Font << /F1 5 0 R >> >>";
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 8 0 R 9 0 R] /Count 3 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] {resources} /Contents 6 0 R /Annots [7 0 R] >>"))
        .stream(&format!("/Type /XObject /Subtype /Image {image_dict}"), data)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .stream("", format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q BT 3 Tr /F1 12 Tf 20 380 Td (Invisible OCR words) Tj ET"))
        .obj("<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /Contents (A comment) >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] {resources} /Contents 10 0 R >>"))
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] {resources} /Contents 11 0 R >>"))
        .stream("", "BT /F1 12 Tf 20 380 Td (Real text) Tj ET")
        .stream("", format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q"))
        .build()
}

fn grey_scan() -> Vec<u8> {
    scan_pdf(&format!("/Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true"), &scan_pixels(1))
}

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("scan.pdf", None, Arc::new(bytes), None).unwrap();
    (s, id)
}

fn object(s: &Session, id: DocId, num: u32) -> Arc<Object> {
    s.get(id).unwrap().editor.as_ref().unwrap().cos.get(ObjRef::new(num, 0))
}

/// The samples of the image page 1 draws, now.
fn samples(s: &Session, id: DocId) -> optimize::Samples {
    let doc = s.get(id).unwrap();
    let object = doc.page_images(0)[0].object.unwrap();
    let cos = &doc.editor.as_ref().unwrap().cos;
    let Object::Stream(img) = &*cos.get(object) else { panic!("the image") };
    optimize::decode_samples(cos, img, &mut pdfcraft_cos::ImageBudget::with_work(u64::MAX)).unwrap()
}

fn grey_at(px: &optimize::Samples, x: usize, y: usize) -> u8 {
    px.data[(y * W + x) * px.components]
}

#[test]
fn a_scan_gets_white_paper_even_edges_dark_strokes_and_no_specks_in_place() {
    let (mut s, id) = open(grey_scan());
    let before = samples(&s, id);
    let kept: Vec<Arc<Object>> = [3, 5, 6, 7, 8, 10, 11].iter().map(|&n| object(&s, id, n)).collect();
    let text_before = s.get(id).unwrap().text_lines(0);
    let original = s.get(id).unwrap().bytes.clone();

    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!(report.pages, vec![0, 2], "the shared image counts for both its pages");
    assert_eq!(report.images, 1);
    assert_eq!(report.not_scanned, vec![1], "the text page is no scan");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Enhance scanned file"));

    let after = samples(&s, id);
    assert_eq!((after.width, after.height, after.components, after.jpeg), (W as u32, H as u32, 1, false));
    // Paper, also in the shadowed gutter and at the right edge, turns (nearly) white.
    for (x, y) in [(150, 40), (150, 380), (3, 40), (3, 380), (30, 200), (W - 2, 40)] {
        assert!(grey_at(&before, x, y) < 210, "the fixture's paper is grey at ({x}, {y})");
        assert!(grey_at(&after, x, y) >= 235, "paper at ({x}, {y}): {} → {}", grey_at(&before, x, y), grey_at(&after, x, y));
    }
    // The edge is evened out: the gutter is as white as the middle.
    assert!(grey_at(&after, 150, 40).abs_diff(grey_at(&after, 3, 40)) <= 12);
    // Strokes stay dark (darker, with the contrast), also in the gutter's light.
    for (x, y) in [(150, 62), (61, 102), (238, 342)] {
        assert!(grey_at(&after, x, y) <= 40, "stroke at ({x}, {y}): {}", grey_at(&after, x, y));
    }
    for (x, y) in SPECKS {
        assert!(grey_at(&before, x, y) < 60);
        assert!(grey_at(&after, x, y) >= 200, "speck at ({x}, {y}): {}", grey_at(&after, x, y));
    }
    // Everything but the image is as it was: the page, its invisible text layer, the comment, the
    // font, the other pages. The image keeps its entries.
    let now: Vec<Arc<Object>> = [3, 5, 6, 7, 8, 10, 11].iter().map(|&n| object(&s, id, n)).collect();
    assert_eq!(kept, now);
    assert_eq!(s.get(id).unwrap().info.pages.len(), 3);
    let text_after = s.get(id).unwrap().text_lines(0);
    assert!(text_after.iter().any(|l| l.text.contains("Invisible OCR words")), "{text_after:?}");
    assert_eq!(text_before.len(), text_after.len());
    let Object::Stream(img) = &*object(&s, id, 4) else { panic!("the image") };
    assert_eq!(img.dict.get(b"Interpolate"), Some(&Object::Bool(true)));
    assert_eq!(img.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));

    // Undo brings the original image, and the original file, back.
    s.undo(id).unwrap();
    assert_eq!(samples(&s, id), before);
    assert_eq!(s.get(id).unwrap().bytes, original);
    assert_eq!(s.get(id).unwrap().can_undo(), None);
}

#[test]
fn a_jpeg_scan_stays_a_jpeg_in_colour() {
    let px = scan_pixels(3);
    let (mut s, id) = open(scan_pdf(&format!("/Width {W} /Height {H} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode"), &jpeg(&px, 3)));
    let report = s.enhance_scanned_pages(id, &[0], ScanCleanSettings::at(80)).unwrap();
    // Only page 1 was asked for; the image it shares with page 3 is changed for both.
    assert_eq!((report.pages.as_slice(), report.images), (&[0][..], 1));
    let after = samples(&s, id);
    assert!(after.jpeg && after.components == 3);
    let Object::Stream(img) = &*object(&s, id, 4) else { panic!("the image") };
    assert_eq!(img.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    // Yellowed paper becomes white; the strokes stay dark.
    let at = |x: usize, y: usize| after.data[(y * W + x) * 3..][..3].to_vec();
    for (x, y) in [(150, 40), (5, 300)] {
        assert!(at(x, y).iter().all(|&v| v >= 230), "paper at ({x}, {y}): {:?}", at(x, y));
    }
    assert!(at(150, 63).iter().all(|&v| v <= 60), "stroke: {:?}", at(150, 63));
    // The file still renders.
    let doc = s.get(id).unwrap();
    let shot = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone()).render(pdfcraft_render::RenderRequest {
        page: 0,
        scale: 1.0,
        ..Default::default()
    });
    assert!(shot.error.is_none(), "{:?}", shot.error);
}

#[test]
fn despeckle_is_its_own_switch() {
    let speck = |settings: ScanCleanSettings| {
        let (mut s, id) = open(grey_scan());
        s.enhance_scanned_pages(id, &[0], settings).unwrap();
        let after = samples(&s, id);
        SPECKS.iter().map(|&(x, y)| grey_at(&after, x, y)).max().unwrap_or(0)
    };
    assert!(speck(ScanCleanSettings { despeckle: false, ..ScanCleanSettings::at(80) }) < 100, "kept without despeckle");
    assert!(speck(ScanCleanSettings::at(80)) >= 200);
    // Despeckle alone, at level 0, changes nothing but the specks.
    let (mut s, id) = open(grey_scan());
    let before = samples(&s, id);
    s.enhance_scanned_pages(id, &[], ScanCleanSettings { level: 0, edge_shadows: false, despeckle: true, ..ScanCleanSettings::default() }).unwrap();
    let after = samples(&s, id);
    let changed: Vec<usize> = before.data.iter().zip(&after.data).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
    let mut specks: Vec<usize> = SPECKS.iter().map(|&(x, y)| y * W + x).collect();
    specks.sort_unstable();
    assert_eq!(changed, specks);
    // Nothing to do: no undo step, and the report says why.
    let (mut s, id) = open(grey_scan());
    let report = s
        .enhance_scanned_pages(id, &[], ScanCleanSettings { level: 0, edge_shadows: false, despeckle: false, ..ScanCleanSettings::default() })
        .unwrap();
    assert_eq!(report.images, 0);
    assert!(report.pages.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].reason.contains("already clean"), "{:?}", report.skipped);
    assert_eq!(s.get(id).unwrap().can_undo(), None);
    assert!(!s.get(id).unwrap().dirty);
}

#[test]
fn a_black_and_white_scan_stays_one_bit_and_loses_its_specks() {
    // 64 × 64, one bit per pixel, 1 = white: a black bar and two specks.
    let (w, h) = (64usize, 64usize);
    let mut bits = vec![0xFFu8; w / 8 * h];
    let mut black = |x: usize, y: usize| bits[y * w / 8 + x / 8] &= !(0x80 >> (x % 8));
    for y in 20..30 {
        for x in 10..50 {
            black(x, y);
        }
    }
    black(5, 5);
    black(58, 58);
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>")
        .stream("/Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceGray /BitsPerComponent 1", &bits)
        .stream("", "q 64 0 0 64 0 0 cm /Im0 Do Q")
        .build();
    let (mut s, id) = open(bytes);
    // Only Despeckle changes a black and white scan: without it the image is reported.
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings { despeckle: false, ..ScanCleanSettings::at(100) }).unwrap();
    assert_eq!(report.images, 0);
    assert!(report.skipped.first().is_some_and(|k| k.reason.contains("black and white")), "{:?}", report.skipped);
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!(report.images, 1);
    let Object::Stream(img) = &*object(&s, id, 4) else { panic!("the image") };
    assert_eq!(img.dict.int(b"BitsPerComponent"), Some(1));
    let px = samples(&s, id);
    assert!(px.bilevel);
    let at = |x: usize, y: usize| px.data[y * w + x];
    assert_eq!((at(5, 5), at(58, 58)), (255, 255), "specks gone");
    assert_eq!((at(10, 20), at(30, 25), at(49, 29)), (0, 0, 0), "the bar stays");
    assert_eq!(px.data.iter().filter(|&&v| v == 0).count(), 40 * 10);
}

/// A layered (mixed-raster) scan: the background picture, then the ink's colours drawn through a
/// mask of the text's shapes. Only the background is paper (#6811 review).
#[test]
fn the_ink_layer_of_a_layered_scan_is_left_as_it_is() {
    let ink = "/Type /XObject /Subtype /Image /Width 300 /Height 400 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let shapes = "/Type /XObject /Subtype /Image /Width 300 /Height 400";
    let page = |ink: u32| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Im0 4 0 R /Ink {ink} 0 R >> >> /Contents 9 0 R >>")
    };
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>")
        .obj(page(5))
        .stream(&format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8"), scan_pixels(1))
        // Page 1's ink goes through a stencil, page 2's through a one-bit soft mask.
        .stream(&format!("{ink} /Mask 6 0 R"), vec![40u8; W * H])
        .stream(&format!("{shapes} /ImageMask true /BitsPerComponent 1"), vec![0xaau8; W.div_ceil(8) * H])
        .obj(page(8))
        .stream(&format!("{ink} /SMask 10 0 R"), vec![40u8; W * H])
        .stream("", format!("q {W} 0 0 {H} 0 0 cm /Im0 Do /Ink Do Q"))
        .stream(&format!("{shapes} /ColorSpace /DeviceGray /BitsPerComponent 1"), vec![0xaau8; W.div_ceil(8) * H])
        .build();
    let (mut s, id) = open(bytes);
    let before = [object(&s, id, 5), object(&s, id, 8)];
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!((report.images, &report.pages), (1, &vec![0, 1]), "the background is cleaned, once");
    let why: Vec<(usize, &str)> = report.skipped.iter().map(|k| (k.pages[0], k.reason.as_str())).collect();
    assert_eq!(why.len(), 2, "{why:?}");
    assert!(why.iter().all(|(_, reason)| reason.contains("stencil mask")), "{why:?}");
    assert_eq!([object(&s, id, 5), object(&s, id, 8)], before, "the ink keeps its colours");
}

/// Contributor-original JPEG 2000: a `w` × `h` codestream of `components` 8-bit components in one
/// tile, every sample 128 (an empty packet decodes to the midpoint).
fn jpx(w: u32, h: u32, components: u16) -> Vec<u8> {
    let mut c = vec![0xff, 0x4f, 0xff, 0x51];
    c.extend_from_slice(&(38 + 3 * components).to_be_bytes());
    c.extend_from_slice(&0_u16.to_be_bytes());
    for v in [w, h, 0, 0, w, h, 0, 0] {
        c.extend_from_slice(&v.to_be_bytes());
    }
    c.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        c.extend_from_slice(&[7, 1, 1]);
    }
    c.extend_from_slice(&[0xff, 0x52, 0, 12, 0, 0, 0, 1, 0, 0, 4, 4, 0, 1]);
    c.extend_from_slice(&[0xff, 0x5c, 0, 4, 0x40, 0x40]);
    c.extend_from_slice(&[0xff, 0x90, 0, 10, 0, 0]);
    c.extend_from_slice(&(14 + u32::from(components)).to_be_bytes());
    c.extend_from_slice(&[0, 1, 0xff, 0x93]);
    c.extend(std::iter::repeat_n(0, usize::from(components)));
    c.extend_from_slice(&[0xff, 0xd9]);
    c
}

#[test]
fn a_jpeg_2000_scan_is_cleaned_and_written_back_as_a_jpeg() {
    // Grey paper, stored as JPEG 2000 with the colour space left to the data.
    let dict = format!("/Width {W} /Height {H} /BitsPerComponent 8 /Filter /JPXDecode");
    let (mut s, id) = open(scan_pdf(&dict, &jpx(W as u32, H as u32, 1)));
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(50)).unwrap();
    assert_eq!((report.images, &report.pages, report.skipped.len()), (1, &vec![0, 2], 0), "{:?}", report.skipped);
    let Object::Stream(img) = &*object(&s, id, 4) else { panic!("the image") };
    assert_eq!(img.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert_eq!(img.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]), "the new encoding names what the data said");
    assert_eq!((img.dict.int(b"Width"), img.dict.int(b"Height")), (Some(W as i64), Some(H as i64)));
    let px = samples(&s, id);
    assert!(grey_at(&px, 150, 200) >= 250, "the grey paper is white: {}", grey_at(&px, 150, 200));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Enhance scanned file"));
}

/// #6811 review: a JPEG 2000 image the decoder panics on (a grey image with a three-column
/// palette) is reported, and the other pages are still cleaned.
#[test]
fn a_jpeg_2000_image_the_decoder_cannot_read_does_not_stop_the_other_pages() {
    let boxed = |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32 + 8).to_be_bytes()[..], kind, data].concat();
    let header = [boxed(b"colr", &[1, 0, 0, 0, 0, 0, 17]), boxed(b"pclr", &[&[1u8, 0, 3, 7, 7, 7][..], &[9u8; 256 * 3]].concat())].concat();
    let hostile = [
        boxed(b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]),
        boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        boxed(b"jp2h", &header),
        boxed(b"jp2c", &jpx(W as u32, H as u32, 1)),
    ]
    .concat();
    let page = |image: u32| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Im0 {image} 0 R >> >> /Contents 7 0 R >>")
    };
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>")
        .obj(page(4))
        .stream(&format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /BitsPerComponent 8 /Filter /JPXDecode"), hostile)
        .obj(page(6))
        .stream(&format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8"), scan_pixels(1))
        .stream("", format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q"))
        .build();
    let (mut s, id) = open(bytes);
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(50)).unwrap();
    assert_eq!((report.images, &report.pages), (1, &vec![1]), "page 2 is cleaned");
    assert_eq!(report.skipped.len(), 1, "{:?}", report.skipped);
    assert_eq!(report.skipped[0].pages, vec![0]);
}

#[test]
fn a_halftone_scan_becomes_clean_grey_whatever_its_fax_encoding() {
    // 96 × 96: grey paper as 20 % dots, a darker block (70 % dots) as the ink; 1 = white.
    let (w, h) = (96usize, 96usize);
    let mut bits = vec![0u8; w / 8 * h];
    for y in 0..h {
        for x in 0..w {
            let ink = (30..66).contains(&x) && (30..66).contains(&y);
            let dot = (x * 7 + y * 3) % 10 < if ink { 7 } else { 2 };
            if !dot {
                bits[y * w / 8 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let g4 = pdfcraft_filters::encode_g4(&bits, w as u32, h as u32).expect("encodable");
    let dict = format!(
        "/Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns {w} /Rows {h} >>"
    );
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 96 96] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>")
        .stream(&format!("/Type /XObject /Subtype /Image {dict}"), &g4)
        .stream("", "q 96 0 0 96 0 0 cm /Im0 Do Q")
        .build();
    let (mut s, id) = open(bytes);
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(50)).unwrap();
    assert_eq!((report.images, report.skipped.len()), (1, 0), "{:?}", report.skipped);
    assert_eq!(report.halftone, vec![HalftoneImage { format: "CCITT fax 1-bit halftone".into(), pages: vec![0] }]);
    let Object::Stream(img) = &*object(&s, id, 4) else { panic!("the image") };
    assert_eq!(img.dict.int(b"BitsPerComponent"), Some(8));
    let px = samples(&s, id);
    assert!(!px.bilevel);
    assert_eq!((px.width, px.height), (96, 96), "the same size");
    let at = |x: usize, y: usize| px.data[y * w + x];
    assert!(at(10, 10) >= 225 && at(85, 85) >= 225, "paper white: {} {}", at(10, 10), at(85, 85));
    assert!(at(48, 48) <= 120, "ink dark: {}", at(48, 48));
}

#[test]
fn hostile_images_are_reported_not_a_panic() {
    // Each page draws one image over the whole page.
    let images: [(&str, Vec<u8>); 6] = [
        ("/Width 100000 /Height 100000 /ColorSpace /DeviceGray /BitsPerComponent 8", vec![0; 64]),
        ("/Width 300 /Height 400 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode", vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 3, 9]),
        ("/Width 300 /Height 400 /ColorSpace /DeviceGray /BitsPerComponent 8", vec![200; 1000]),
        ("/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", vec![128]),
        ("/Width 8 /Height 8 /ColorSpace /DeviceCMYK /BitsPerComponent 8", vec![0; 256]),
        ("/Width 300 /Height 400 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask [250 255]", vec![200; 300 * 400]),
    ];
    let n = images.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + 3 * i)).collect();
    let mut pdf = Pdf::new().obj("<< /Type /Catalog /Pages 2 0 R >>").obj(format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>"));
    for (i, (dict, data)) in images.iter().enumerate() {
        let (img, contents) = (4 + 3 * i, 5 + 3 * i);
        pdf = pdf
            .obj(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /XObject << /Im0 {img} 0 R >> >> /Contents {contents} 0 R >>"
            ))
            .stream(&format!("/Type /XObject /Subtype /Image {dict}"), data)
            .stream("", "q 300 0 0 400 0 0 cm /Im0 Do Q");
    }
    let (mut s, id) = open(pdf.build());
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!(report.images, 0);
    assert_eq!(report.not_scanned, vec![3], "a 1 × 1 image is no scan");
    let why: Vec<(usize, &str)> = report.skipped.iter().map(|k| (k.pages[0], k.reason.as_str())).collect();
    assert_eq!(why.len(), 5, "{why:?}");
    assert!(why[0].0 == 0 && why[0].1.contains("80 million"), "{why:?}");
    assert!(why[1].0 == 1 && why[1].1.contains("could not be decoded"), "{why:?}");
    assert!(why[2].0 == 2 && why[2].1.contains("shorter"), "{why:?}");
    assert!(why[3].0 == 4 && why[3].1.contains("colour space"), "{why:?}");
    assert!(why[4].0 == 5 && why[4].1.contains("colour-key mask"), "{why:?}");
    assert_eq!(s.get(id).unwrap().can_undo(), None);
    // Pages that don't exist are an error, not a panic.
    assert!(matches!(s.enhance_scanned_pages(id, &[99], ScanCleanSettings::default()), Err(EditError::Invalid(_))));
}

#[test]
fn a_scan_drawn_inside_a_form_xobject_is_reported_not_called_no_scan() {
    // Pages 1 and 3 draw the scan through Form Fm0 (as some scanners write it), page 2 is text.
    let page = |contents: usize| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Fm0 4 0 R >> /Font << /F1 9 0 R >> >> /Contents {contents} 0 R >>"
        )
    };
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 7 0 R 10 0 R] /Count 3 >>")
        .obj(page(6))
        .stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 {W} {H}] /Resources << /XObject << /Im0 5 0 R >> >>"),
            format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q"),
        )
        .stream(&format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8"), scan_pixels(1))
        .stream("", "/Fm0 Do")
        .obj(page(8))
        .stream("", "BT /F1 12 Tf 20 380 Td (Real text) Tj ET")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(page(11))
        .stream("", "q /Fm0 Do Q")
        .build();
    let (mut s, id) = open(bytes);
    let before = object(&s, id, 5);
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert!(report.pages.is_empty() && report.images == 0, "{report:?}");
    assert_eq!(report.not_scanned, vec![1], "only the text page is no scan");
    assert_eq!(report.skipped.len(), 1, "the form is reported once: {report:?}");
    let k = &report.skipped[0];
    assert_eq!((k.pages.as_slice(), k.name.as_str()), (&[0, 2][..], "Fm0"));
    assert!(k.reason.contains("form XObject"), "{k:?}");
    assert_eq!(object(&s, id, 5), before, "the picture is left as it was");
    assert_eq!(s.get(id).unwrap().can_undo(), None);

    // A page of vector artwork in a Form, holding only a small logo, is no scan.
    let logo = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Fm0 4 0 R >> >> /Contents 6 0 R >>"))
        .stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 {W} {H}] /Resources << /XObject << /Im0 5 0 R >> >>"),
            format!("0 0 {W} {H} re f q 40 0 0 20 10 370 cm /Im0 Do Q"),
        )
        .stream("/Type /XObject /Subtype /Image /Width 40 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8", [128; 800])
        .stream("", "/Fm0 Do")
        .build();
    let (mut s, id) = open(logo);
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!((report.not_scanned.as_slice(), report.skipped.len()), (&[0][..], 0), "{report:?}");
}

#[test]
fn a_page_mixing_its_own_image_and_a_scan_in_a_form_cleans_the_one_and_reports_the_other() {
    // Page 1: its own image covers 40% and a Form holding a scan 20%: a scan only together.
    // Page 2: its own image covers the page and the Form 20% more. Both pages clean Im1 and
    // report Fm0.
    let page = |contents: usize| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Fm0 4 0 R /Im1 6 0 R >> >> /Contents {contents} 0 R >>"
        )
    };
    let gray = format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8");
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>")
        .obj(page(7))
        .stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 {W} {H}] /Resources << /XObject << /Im0 5 0 R >> >>"),
            format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q"),
        )
        .stream(&gray, scan_pixels(1))
        .stream(&gray, scan_pixels(1))
        .stream("", format!("q {W} 0 0 160 0 240 cm /Im1 Do Q q 1 0 0 0.2 0 0 cm /Fm0 Do Q"))
        .obj(page(9))
        .stream("", format!("q {W} 0 0 {H} 0 0 cm /Im1 Do Q q 1 0 0 0.2 0 0 cm /Fm0 Do Q"))
        .build();
    let (mut s, id) = open(bytes);
    let (form_picture, own) = (object(&s, id, 5), object(&s, id, 6));
    let report = s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap();
    assert_eq!((report.pages.as_slice(), report.images), (&[0, 1][..], 1), "Im1 is cleaned on both pages: {report:?}");
    assert!(report.not_scanned.is_empty(), "{report:?}");
    assert_eq!(report.skipped.len(), 1, "the form is reported once: {report:?}");
    let k = &report.skipped[0];
    assert_eq!((k.pages.as_slice(), k.name.as_str()), (&[0, 1][..], "Fm0"));
    assert!(k.reason.contains("form XObject"), "{k:?}");
    assert_ne!(object(&s, id, 6), own, "the page's own image is cleaned");
    assert_eq!(object(&s, id, 5), form_picture, "the picture in the form is left as it was");
}

#[test]
fn protected_documents_keep_their_encryption_and_restrictions() {
    let protect = |user: &str, permissions: i32| {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(grey_scan())).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: user,
            owner_password: "owner",
            permissions,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap())
    };
    // Opened with the password: cleaned, saved still encrypted, and the clean scan reads back.
    let mut s = Session::new();
    let id = s.open("x.pdf", None, protect("pw", -1), Some("pw")).unwrap();
    assert_eq!(s.enhance_scanned_pages(id, &[], ScanCleanSettings::at(100)).unwrap().images, 1);
    let saved = s.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(saved.clone()).is_err(), "still protected");
    let id2 = s.open("y.pdf", None, saved, Some("pw")).unwrap();
    assert!(grey_at(&samples(&s, id2), 3, 40) >= 235);
    // Changes withheld: refused before any work.
    let id = s.open("r.pdf", None, protect("", 0b0100), None).unwrap();
    assert_eq!(s.enhance_scanned_pages(id, &[], ScanCleanSettings::default()).unwrap_err(), EditError::NotPermitted("changes to the document"));
    assert!(!s.get(id).unwrap().dirty);
}

#[test]
fn a_result_for_an_older_state_is_refused() {
    let (mut s, id) = open(grey_scan());
    let job = s.scan_clean_job(id, &[], ScanCleanSettings::at(70)).unwrap();
    assert_eq!(job.total(), 1);
    let cleaned = job.run(|_, _| true).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    assert!(matches!(s.apply_scan_clean(id, cleaned), Err(EditError::Invalid(_))));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Rotate page"));
    // Cancelled from the progress callback.
    let job = s.scan_clean_job(id, &[], ScanCleanSettings::at(70)).unwrap();
    assert!(matches!(job.run(|_, _| false), Err(EditError::Cancelled)));
}

#[test]
fn edge_shadow_removal_whitens_the_gutter_background_removal_alone_leaves() {
    let gutter = |edge_shadows: bool| {
        let (mut s, id) = open(grey_scan());
        s.enhance_scanned_pages(id, &[0], ScanCleanSettings { edge_shadows, ..ScanCleanSettings::at(100) }).unwrap();
        let after = samples(&s, id);
        [(3, 40), (3, 380), (10, 200)].map(|(x, y)| grey_at(&after, x, y))
    };
    assert!(gutter(false).iter().any(|&v| v < 200), "{:?}", gutter(false));
    assert!(gutter(true).iter().all(|&v| v >= 235), "{:?}", gutter(true));
    // On its own, at level 0, it lightens the gutter's paper to the paper beside it.
    let (mut s, id) = open(grey_scan());
    let before = samples(&s, id);
    s.enhance_scanned_pages(id, &[0], ScanCleanSettings { level: 0, edge_shadows: true, despeckle: false, ..ScanCleanSettings::default() }).unwrap();
    let after = samples(&s, id);
    assert!(grey_at(&after, 3, 40).abs_diff(grey_at(&before, 150, 40)) <= 4, "{} vs {}", grey_at(&after, 3, 40), grey_at(&before, 150, 40));
    assert_eq!(grey_at(&after, 150, 40), grey_at(&before, 150, 40), "the middle is left alone");
}

/// Two pages, each with its own scan image (objects 4 and 7), so cleaning one page can be told
/// apart from cleaning the other.
fn two_scans_pdf() -> Vec<u8> {
    let dict = format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8");
    let draw = format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q");
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"))
        .stream(&dict, scan_pixels(1))
        .stream("", &draw)
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Im0 7 0 R >> >> /Contents 8 0 R >>"))
        .stream(&dict, scan_pixels(1))
        .stream("", &draw)
        .build()
}

#[test]
fn only_the_chosen_pages_scan_is_cleaned() {
    // Clean page 1, then page 2 of a fresh copy: the other page's image stays byte-identical.
    for (chosen, image, other) in [(0usize, 4u32, 7u32), (1, 7, 4)] {
        let (mut s, id) = open(two_scans_pdf());
        let (image_before, other_before) = (object(&s, id, image), object(&s, id, other));
        assert_eq!(s.get(id).unwrap().page_images(chosen)[0].object, Some(ObjRef::new(image, 0)), "the fixture");
        let report = s.enhance_scanned_pages(id, &[chosen], ScanCleanSettings::at(100)).unwrap();
        assert_eq!((report.pages.as_slice(), report.images), (&[chosen][..], 1), "page {}", chosen + 1);
        assert_ne!(object(&s, id, image), image_before, "page {}'s image was not cleaned", chosen + 1);
        assert_eq!(object(&s, id, other), other_before, "cleaning page {} changed the other page's image", chosen + 1);
        let (Object::Stream(was), Object::Stream(now)) = (&*other_before, &*object(&s, id, other)) else { panic!("the images") };
        assert_eq!(was.raw, now.raw, "the other image's stream bytes");
        assert_eq!(s.get(id).unwrap().can_undo(), Some("Enhance scanned file"));
    }
}

#[test]
fn a_pages_own_halftone_tuning_changes_only_that_pages_picture() {
    // Two pages, each with its own copy of the same 96 x 96 halftone scan.
    let (w, h) = (96usize, 96usize);
    let mut bits = vec![0u8; w / 8 * h];
    for y in 0..h {
        for x in 0..w {
            let ink = (30..66).contains(&x) && (30..66).contains(&y);
            if (x * 7 + y * 3) % 10 >= if ink { 7 } else { 2 } {
                bits[y * w / 8 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let image = "/Type /XObject /Subtype /Image /Width 96 /Height 96 /ColorSpace /DeviceGray /BitsPerComponent 1";
    let page = |img: u32, contents: u32| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 96 96] /Resources << /XObject << /Im0 {img} 0 R >> >> /Contents {contents} 0 R >>")
    };
    let bytes = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>")
        .obj(page(5, 7))
        .obj(page(6, 8))
        .stream(image, &bits)
        .stream(image, &bits)
        .stream("", "q 96 0 0 96 0 0 cm /Im0 Do Q")
        .stream("", "q 96 0 0 96 0 0 cm /Im0 Do Q")
        .build();
    let (mut s, id) = open(bytes);
    let heavy = HalftoneTuning { weight: 100, contrast: 100, cleanup: 50 };
    let light = HalftoneTuning { weight: 0, contrast: 0, cleanup: 50 };
    let job = s.scan_clean_job(id, &[], ScanCleanSettings::default()).unwrap().with_page_tunings(vec![(0, heavy), (1, light)]);
    let cleaned = job.run(|_, _| true).unwrap();
    assert_eq!(cleaned.report.halftone.len(), 2, "both pages restored");
    s.apply_scan_clean(id, cleaned).unwrap();
    let ink = |page: usize| {
        let doc = s.get(id).unwrap();
        let cos = &doc.editor.as_ref().unwrap().cos;
        let Object::Stream(img) = &*cos.get(doc.page_images(page)[0].object.unwrap()) else { panic!("the image") };
        let px = optimize::decode_samples(cos, img, &mut pdfcraft_cos::ImageBudget::with_work(u64::MAX)).unwrap();
        px.data.iter().map(|v| u32::from(255 - v)).sum::<u32>()
    };
    assert!(ink(0) > ink(1), "page 1 was tuned heavier than page 2: {} vs {}", ink(0), ink(1));
}
