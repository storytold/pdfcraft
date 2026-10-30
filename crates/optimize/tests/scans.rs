//! Recognize Text's Searchable Image output: scan images downsampled in place.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_optimize::downsample_page_images;

/// Photo-like samples: a gradient with a little texture.
fn samples(w: u32, h: u32, n: usize) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            px.push(((x * 255 / w.max(1)) as u8).wrapping_add(((x ^ y) & 7) as u8));
            if n == 3 {
                px.push((y * 255 / h.max(1)) as u8);
                px.push(128);
            }
        }
    }
    px
}

fn image_dict(w: u32, h: u32, bpc: i64, cs: Option<Object>) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
    if let Some(cs) = cs {
        d.set(b"ColorSpace".to_vec(), cs);
    }
    d
}

fn jpeg_image(doc: &mut Document, w: u32, h: u32, n: usize, extra: impl FnOnce(&mut Dict)) -> ObjRef {
    let mut d = image_dict(w, h, 8, Some(Object::name(if n == 1 { "DeviceGray" } else { "DeviceRGB" })));
    extra(&mut d);
    let mut out = Vec::new();
    let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode(&samples(w, h, n), w, h, ty).unwrap();
    d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    doc.add(Object::Stream(Stream::from_raw(d, out)))
}

/// A black-and-white "scan": text-like bars, 1 bit per pixel, Flate.
fn bilevel(w: u32, h: u32) -> Vec<u8> {
    let row = w.div_ceil(8) as usize;
    let mut out = vec![0u8; row * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            if (y / 8) % 3 == 0 && (x / 6) % 2 == 0 {
                out[y * row + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    out
}

fn page(doc: &mut Document, xobjects: &[(&str, ObjRef)], content: &str) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut x = Dict::new();
    for (n, r) in xobjects {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

fn stream(doc: &Document, r: ObjRef) -> Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        _ => panic!("not a stream"),
    }
}

/// An RGB JPEG scan drawn at 600 ppi (1200 px over 2 in) with a soft mask and a decode array:
/// downsampled to 300 it is 600 px, still a JPEG in the same object, colour space, decode array
/// and mask untouched, and smaller.
#[test]
fn a_600_ppi_jpeg_is_halved_in_place_and_keeps_its_kind() {
    let mut doc = Document::new_empty();
    let mask = doc.add(Object::Stream(Stream::flate(image_dict(1200, 1200, 8, Some(Object::name("DeviceGray"))), &vec![200; 1200 * 1200])));
    let img = jpeg_image(&mut doc, 1200, 1200, 3, |d| {
        d.set(b"SMask".to_vec(), Object::Ref(mask));
        d.set(b"Decode".to_vec(), Object::Array([1, 0, 0, 1, 0, 1].into_iter().map(Object::Int).collect()));
        d.set(b"Interpolate".to_vec(), Object::Bool(true));
    });
    let p = page(&mut doc, &[("Im0", img)], "q 144 0 0 144 36 36 cm /Im0 Do Q");
    let before = stream(&doc, img);
    let mask_before = stream(&doc, mask);

    let r = downsample_page_images(&mut doc, &[p], 300.0).unwrap();
    assert_eq!((r.images, r.resampled), (1, 1), "{r:?}");
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    let after = stream(&doc, img);
    assert_eq!((after.dict.int(b"Width"), after.dict.int(b"Height")), (Some(600), Some(600)));
    assert_eq!(after.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    for key in [&b"ColorSpace"[..], b"Decode", b"SMask", b"Interpolate", b"BitsPerComponent"] {
        assert_eq!(after.dict.get(key), before.dict.get(key), "{}", String::from_utf8_lossy(key));
    }
    assert!(after.raw.len() < before.raw.len() / 2, "{} -> {}", before.raw.len(), after.raw.len());
    assert_eq!((r.bytes_before, r.bytes_after), (before.raw.len(), after.raw.len()));
    // The soft mask is mapped onto the image's unit square whatever its size: left alone.
    assert_eq!(stream(&doc, mask).raw, mask_before.raw);
    // The JPEG decodes at its new size.
    let decoded = image::load_from_memory_with_format(&after.raw, image::ImageFormat::Jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (600, 600));
}

/// Black and white stays 1-bit and lossless (Flate), with its decode array; an image mask too.
#[test]
fn black_and_white_scans_stay_one_bit_and_lossless() {
    let mut doc = Document::new_empty();
    let mut d = image_dict(1200, 1600, 1, Some(Object::name("DeviceGray")));
    d.set(b"Decode".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(0)]));
    let scan = doc.add(Object::Stream(Stream::flate(d, &bilevel(1200, 1600))));
    let mut m = image_dict(1200, 1600, 1, None);
    m.set(b"ImageMask".to_vec(), Object::Bool(true));
    let stencil = doc.add(Object::Stream(Stream::flate(m, &bilevel(1200, 1600))));
    // 1200 px over 2 in and 1600 over 2.67 in: 600 ppi each.
    let p = page(&mut doc, &[("S", scan), ("M", stencil)], "q 144 0 0 192 0 0 cm /S Do Q q 0 g 144 0 0 192 200 0 cm /M Do Q");
    let r = downsample_page_images(&mut doc, &[p], 300.0).unwrap();
    assert_eq!((r.images, r.resampled), (2, 2), "{r:?}");
    for (img, decode) in [(scan, true), (stencil, false)] {
        let s = stream(&doc, img);
        assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height"), s.dict.int(b"BitsPerComponent")), (Some(600), Some(800), Some(1)));
        assert_eq!(s.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
        assert_eq!(s.dict.contains(b"Decode"), decode);
        let data = s.decoded().unwrap();
        assert_eq!(data.len(), 600usize.div_ceil(8) * 800);
        // The bars survive: some ink, mostly paper.
        let ink = data.iter().map(|b| b.count_ones() as usize).sum::<usize>();
        assert!(ink > 600 * 800 / 20 && ink < 600 * 800 / 2, "{ink}");
    }
    assert_eq!(stream(&doc, stencil).dict.get(b"ImageMask"), Some(&Object::Bool(true)));
}

/// Only the pages asked for, and an image drawn larger elsewhere is measured there; one already
/// at the target is left alone, and so is everything when the target makes no sense.
#[test]
fn only_finer_images_on_the_pages_asked_for_change() {
    let mut doc = Document::new_empty();
    let shared = jpeg_image(&mut doc, 1200, 1200, 1, |_| {});
    let other = jpeg_image(&mut doc, 1200, 1200, 1, |_| {});
    let at_target = jpeg_image(&mut doc, 600, 600, 1, |_| {});
    // Page 1 draws `shared` at 600 ppi and `at_target` at 300; page 2 draws `shared` at 300
    // (4 in) and `other` at 600.
    let p1 = page(&mut doc, &[("A", shared), ("T", at_target)], "q 144 0 0 144 0 0 cm /A Do Q q 144 0 0 144 200 0 cm /T Do Q");
    let p2 = page(&mut doc, &[("A", shared), ("B", other)], "q 288 0 0 288 0 0 cm /A Do Q q 144 0 0 144 300 0 cm /B Do Q");
    let before: Vec<Stream> = [shared, other, at_target].iter().map(|r| stream(&doc, *r)).collect();
    let r = downsample_page_images(&mut doc, &[p1], 300.0).unwrap();
    assert_eq!((r.images, r.resampled), (2, 0), "{r:?}");
    for (r, b) in [shared, other, at_target].iter().zip(&before) {
        assert_eq!(stream(&doc, *r).raw, b.raw);
    }
    for target in [f64::NAN, f64::INFINITY, 0.0, -300.0] {
        let r = downsample_page_images(&mut doc, &[p2], target).unwrap();
        assert_eq!(r.resampled, 0, "{target}");
        assert_eq!(stream(&doc, other).raw, before[1].raw);
    }
    // Page 2 alone: `other` is reduced; `shared` is at 300 ppi on page 2.
    let r = downsample_page_images(&mut doc, &[p2], 300.0).unwrap();
    assert_eq!(r.resampled, 1, "{r:?}");
    assert_eq!(stream(&doc, other).dict.int(b"Width"), Some(600));
    assert_eq!(stream(&doc, shared).raw, before[0].raw);
}

/// Images that can't be handled faithfully are left exactly as they are and reported, each
/// with why, without a panic: CMYK, indexed, 16-bit, colour-key masks, premultiplied soft
/// masks, CCITT and JPEG 2000 data, short or broken data and impossible sizes.
#[test]
fn what_cannot_be_handled_is_left_alone_and_reported() {
    let mut doc = Document::new_empty();
    let flate = |d: Dict, len: usize| Object::Stream(Stream::flate(d, &vec![7u8; len]));
    let (w, h) = (1200u32, 1200u32);
    let px = (w * h) as usize;
    let mut cases: Vec<(&str, ObjRef)> = Vec::new();
    cases.push(("CMYK", doc.add(flate(image_dict(w, h, 8, Some(Object::name("DeviceCMYK"))), px * 4))));
    let indexed = Object::Array(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        Object::Int(1),
        Object::String(pdfcraft_cos::PdfString::literal(vec![0u8; 6])),
    ]);
    cases.push(("indexed", doc.add(flate(image_dict(w, h, 8, Some(indexed)), px))));
    cases.push(("16-bit", doc.add(flate(image_dict(w, h, 16, Some(Object::name("DeviceGray"))), px * 2))));
    let mut keyed = image_dict(w, h, 8, Some(Object::name("DeviceGray")));
    keyed.set(b"Mask".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(10)]));
    cases.push(("colour key", doc.add(flate(keyed, px))));
    let mut matte = image_dict(w, h, 8, Some(Object::name("DeviceGray")));
    matte.set(b"Matte".to_vec(), Object::Array(vec![Object::Int(0)]));
    let matte = doc.add(flate(matte, px));
    let mut pre = image_dict(w, h, 8, Some(Object::name("DeviceGray")));
    pre.set(b"SMask".to_vec(), Object::Ref(matte));
    cases.push(("matte", doc.add(flate(pre, px))));
    let mut ccitt = image_dict(w, h, 1, Some(Object::name("DeviceGray")));
    ccitt.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
    cases.push(("CCITT", doc.add(Object::Stream(Stream::from_raw(ccitt, vec![0; 64])))));
    let mut jpx = image_dict(w, h, 8, Some(Object::name("DeviceRGB")));
    jpx.set(b"Filter".to_vec(), Object::name("JPXDecode"));
    cases.push(("JPEG 2000", doc.add(Object::Stream(Stream::from_raw(jpx, vec![0; 64])))));
    cases.push(("short", doc.add(flate(image_dict(w, h, 8, Some(Object::name("DeviceGray"))), 10))));
    cases.push(("short 1-bit", doc.add(flate(image_dict(w, h, 1, Some(Object::name("DeviceGray"))), 10))));
    let mut broken = image_dict(w, h, 8, Some(Object::name("DeviceGray")));
    broken.set(b"Filter".to_vec(), Object::name("FlateDecode"));
    cases.push(("broken", doc.add(Object::Stream(Stream::from_raw(broken, b"not zlib".to_vec())))));
    let mut broken_jpeg = image_dict(w, h, 8, Some(Object::name("DeviceRGB")));
    broken_jpeg.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    cases.push(("broken JPEG", doc.add(Object::Stream(Stream::from_raw(broken_jpeg, vec![0xFF, 0xD8, 0xFF, 0xD9])))));
    let mut huge = image_dict(1, 1, 8, Some(Object::name("DeviceGray")));
    huge.set(b"Width".to_vec(), Object::Int(i64::MAX));
    huge.set(b"Height".to_vec(), Object::Int(i64::MAX));
    cases.push(("huge", doc.add(flate(huge, 10))));
    let mut negative = image_dict(1, 1, 8, Some(Object::name("DeviceGray")));
    negative.set(b"Width".to_vec(), Object::Int(-5));
    cases.push(("negative", doc.add(flate(negative, 10))));
    let mut no_bpc = image_dict(w, h, 8, Some(Object::name("DeviceGray")));
    no_bpc.remove(b"BitsPerComponent");
    cases.push(("no bpc", doc.add(flate(no_bpc, px))));

    let names: Vec<String> = (0..cases.len()).map(|i| format!("I{i}")).collect();
    let xobjects: Vec<(&str, ObjRef)> = names.iter().map(String::as_str).zip(cases.iter().map(|c| c.1)).collect();
    // Each drawn at 2 in: 600 ppi for the 1200-px ones.
    let content: String = names.iter().map(|n| format!("q 144 0 0 144 0 0 cm /{n} Do Q\n")).collect();
    let p = page(&mut doc, &xobjects, &content);
    let before: Vec<Stream> = cases.iter().map(|(_, r)| stream(&doc, *r)).collect();
    let r = downsample_page_images(&mut doc, &[p], 300.0).unwrap();
    assert_eq!(r.resampled, 0, "{r:?}");
    for ((what, img), b) in cases.iter().zip(&before) {
        let after = stream(&doc, *img);
        assert_eq!((&after.dict, &after.raw), (&b.dict, &b.raw), "{what} changed");
    }
    // Every finer one is reported, except the sizes no resolution can be measured for (a
    // negative width is no image at all).
    assert!(r.skipped.len() >= cases.len() - 2, "{:#?}", r.skipped);
    let said = r.skipped.join("\n");
    for reason in ["colour space", "bits per component", "colour-key mask", "/Matte", "CCITTFaxDecode", "JPXDecode", "decoded"] {
        assert!(said.contains(reason), "no {reason:?} in\n{said}");
    }
}
