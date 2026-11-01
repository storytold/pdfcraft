//! The optimizer on documents with images drawn at known sizes.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_filters::{Filter, Params};
use pdfcraft_optimize::{
    Compatibility, Compression, ImageSettings, MonoCompression, MonoSettings, OptimizeError, Settings, SpaceCategory, audit_space,
    effective_resolutions, optimize,
};

fn image(doc: &mut Document, w: u32, h: u32, n: usize, jpeg: bool, smask: Option<ObjRef>) -> ObjRef {
    // A smooth gradient with a little texture (photo-like).
    let mut px = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            let v = ((x * 255 / w.max(1)) as u8).wrapping_add(((x ^ y) & 7) as u8);
            px.push(v);
            if n == 3 {
                px.push((y * 255 / h.max(1)) as u8);
                px.push(128);
            }
        }
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name(if n == 1 { "DeviceGray" } else { "DeviceRGB" }));
    if let Some(m) = smask {
        d.set(b"SMask".to_vec(), Object::Ref(m));
    }
    let s = if jpeg {
        let mut out = Vec::new();
        let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98).encode(&px, w, h, ty).unwrap();
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, out)
    } else {
        Stream::flate(d, &px)
    };
    doc.add(Object::Stream(s))
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
    p.set(b"Thumb".to_vec(), Object::Ref(c));
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

#[test]
fn image_resolution_uses_the_pages_physical_unit() {
    for (unit, scale) in [
        (None, 1.0),
        (Some(Object::Int(10)), 10.0),
        (Some(Object::Real(2.5)), 2.5),
        (Some(Object::Int(0)), 1.0),
        (Some(Object::Int(-10)), 1.0),
        (Some(Object::Real(0.5)), 1.0),
        (Some(Object::name("Ten")), 1.0),
        (Some(Object::Int(100_000)), pdfcraft_model::MAX_USER_UNIT),
    ] {
        for indirect in [false, true] {
            let mut doc = Document::new_empty();
            let img = image(&mut doc, 720, 720, 1, false, None);
            let p = page(&mut doc, &[("Im", img)], "q 72 0 0 72 0 0 cm /Im Do Q");
            let parent = doc.get(p).as_dict().unwrap().reference(b"Parent").unwrap();
            // UserUnit is page-local, unlike Resources and MediaBox.
            doc.update_dict(parent, |d| d.set(b"UserUnit".to_vec(), Object::Int(10))).unwrap();
            if let Some(unit) = unit.clone() {
                let unit = if indirect { Object::Ref(doc.add(unit)) } else { unit };
                doc.update_dict(p, |d| d.set(b"UserUnit".to_vec(), unit)).unwrap();
            }
            let ppi = effective_resolutions(&doc, &[p])[&img];
            assert!((ppi - 720.0 / scale).abs() < 1e-9, "unit={unit:?}, indirect={indirect}: {ppi}");
        }
    }
}

#[test]
fn optimization_retains_an_image_below_threshold_on_a_scaled_page() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 720, 720, 3, false, None);
    let p = page(&mut doc, &[("Im", img)], "q 72 0 0 72 0 0 cm /Im Do Q");
    doc.update_dict(p, |d| d.set(b"UserUnit".to_vec(), Object::Int(10))).unwrap();
    // 720 pixels across ten inches is 72 ppi, already below the 225 ppi threshold.
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    let image = stream(&doc, img);
    assert_eq!((image.dict.int(b"Width"), image.dict.int(b"Height")), (Some(720), Some(720)));
    let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let dimensions: Vec<_> = reopened
        .scan_objects()
        .filter_map(|(_, object)| match &*object {
            Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => Some((s.dict.int(b"Width"), s.dict.int(b"Height"))),
            _ => None,
        })
        .collect();
    assert_eq!(dimensions, [(Some(720), Some(720))]);
}

#[test]
fn images_are_measured_where_drawn_and_downsampled() {
    let mut doc = Document::new_empty();
    // 1200 px drawn 144 pt (2 in) wide → 600 ppi.
    let big = image(&mut doc, 1200, 1200, 3, false, None);
    // 100 px at 144 pt → 50 ppi: left at its size.
    let small = image(&mut doc, 100, 100, 1, false, None);
    // With a soft mask, 800 px at 72 pt → 800 ppi.
    let mask = image(&mut doc, 800, 800, 1, false, None);
    let masked = image(&mut doc, 800, 800, 3, false, Some(mask));
    // A high-quality JPEG at 72 ppi: recompressed, not resized.
    let photo = image(&mut doc, 600, 400, 3, true, None);
    // Drawn twice: 300 ppi and 75 ppi → effective 75 (never downsampled below what a use needs).
    let twice = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im1", big), ("Im2", small)], "q 144 0 0 144 36 600 cm /Im1 Do Q q 144 0 0 144 300 600 cm /Im2 Do Q");
    page(
        &mut doc,
        &[("A", masked), ("B", photo), ("C", twice)],
        "q 72 0 0 72 36 36 cm /A Do Q q 1 0 0 1 100 100 cm 600 0 0 400 0 0 cm /B Do Q q 72 0 0 72 0 0 cm /C Do Q q 288 0 0 288 300 300 cm /C Do Q",
    );
    let pages = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &pages);
    assert_eq!(ppi[&big].round(), 600.0);
    assert_eq!(ppi[&small].round(), 50.0);
    assert_eq!(ppi[&masked].round(), 800.0);
    assert_eq!(ppi[&photo].round(), 72.0);
    assert_eq!(ppi[&twice].round(), 75.0, "the largest use decides");

    let before = write_full(&doc, &SaveOptions::default()).unwrap().len();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images, 5, "{report:?}");
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert!(report.thumbnails == 2);
    let b = stream(&doc, big);
    assert_eq!((b.dict.int(b"Width"), b.dict.int(b"Height"), b.dict.name(b"Filter")), (Some(300), Some(300), Some(&b"DCTDecode"[..])));
    let m = stream(&doc, masked);
    let sm = stream(&doc, mask);
    assert_eq!((m.dict.int(b"Width"), sm.dict.int(b"Width")), (Some(150), Some(150)), "the soft mask follows its image");
    assert_eq!(sm.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(stream(&doc, small).dict.int(b"Width"), Some(100));
    assert_eq!(stream(&doc, twice).dict.int(b"Width"), Some(300), "75 ppi is below the threshold");
    let p = stream(&doc, photo);
    assert_eq!(p.dict.int(b"Width"), Some(600));
    let after_bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(after_bytes.len() * 3 < before, "{} → {}", before, after_bytes.len());

    // The optimized file opens, and the page still shows the picture where it was.
    let reopened = Document::open(Arc::new(after_bytes.clone())).unwrap();
    assert_eq!(pdfcraft_annot::page_refs(&reopened).unwrap().len(), 2);
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(after_bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    // Top-left image area (36..180 x 600..744 user → y from the top 48..192): drawn, not white.
    let i = ((100 * out.width + 100) * 4) as usize;
    assert_ne!(&out.rgba[i..i + 3], &[255, 255, 255]);
}

#[test]
fn an_identity_decode_array_does_not_stop_reduce() {
    // Office scanners write /Decode [0 1 0 1 0 1] (no change) on their page images (#490).
    let with_decode = |doc: &mut Document, r: ObjRef, decode: Vec<Object>| {
        let mut s = stream(doc, r);
        s.dict.set(b"Decode".to_vec(), Object::Array(decode));
        doc.set(r, Object::Stream(s));
    };
    let mut doc = Document::new_empty();
    // 900 px drawn 216 pt (3 in) wide → 300 ppi, like a scanned page.
    let scan = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, scan, [0, 1, 0, 1, 0, 1].map(Object::Int).to_vec());
    let gray = image(&mut doc, 900, 900, 1, false, None);
    with_decode(&mut doc, gray, vec![Object::Real(0.0), Object::Real(1.0)]);
    // A decode array that inverts the colours, and one of the wrong length: left alone.
    let inverted = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, inverted, [1, 0, 1, 0, 1, 0].map(Object::Int).to_vec());
    let odd = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, odd, [0, 1].map(Object::Int).to_vec());
    page(
        &mut doc,
        &[("S", scan), ("G", gray), ("I", inverted), ("O", odd)],
        "q 216 0 0 216 0 0 cm /S Do Q q 216 0 0 216 216 0 cm /G Do Q q 216 0 0 216 0 300 cm /I Do Q q 216 0 0 216 216 300 cm /O Do Q",
    );
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert_eq!(stream(&doc, scan).dict.int(b"Width"), Some(450));
    assert_eq!(stream(&doc, gray).dict.int(b"Width"), Some(450));
    assert!(!stream(&doc, scan).dict.contains(b"Decode"), "the default decode array isn't carried over");
    assert_eq!(stream(&doc, inverted).dict.int(b"Width"), Some(900));
    assert_eq!(stream(&doc, odd).dict.int(b"Width"), Some(900));
}

/// Rows of `w` 1-bit samples (1 white, 0 black), each starting on a byte: lines of text-like
/// glyphs; or, with `noise`, random bits.
fn bilevel(w: u32, h: u32, noise: bool) -> Vec<u8> {
    let stride = (w as usize).div_ceil(8);
    let mut data = vec![0xFF; stride * h as usize];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut black = |x: usize, y: usize| {
        if x < w as usize && y < h as usize {
            data[y * stride + x / 8] &= !(0x80 >> (x % 8));
        }
    };
    if noise {
        for y in 0..h as usize {
            for x in 0..w as usize {
                if next() & 1 == 0 {
                    black(x, y);
                }
            }
        }
        return data;
    }
    // Lines 24 rows apart, of glyphs drawn from a few shapes (a stem, a bowl, an arch, a dotted
    // stem) at any column, 1–3 columns apart with a wider gap between words: text as a scanner
    // sees it. Clean, identical rows would favour Flate; a scan's ragged edges favour Group 4.
    for line in (4..h as usize).step_by(24) {
        let mut x = 8;
        while x + 16 < w as usize {
            let wide = 5 + (next() % 4) as usize;
            let shape = next() % 4;
            // Now and then a row of a stroke is a column wider or narrower at an edge, as a scan's
            // edges are.
            let mut ragged = || match next() % 8 {
                0 => 0,
                1 => 2,
                _ => 1,
            };
            let mut fill = |x0: usize, x1: usize, y0: usize, y1: usize| {
                for y in line + y0..line + y1 {
                    let (left, right) = ((x + x0 + 1).saturating_sub(ragged()), x + x1 + ragged());
                    for xx in left..right.saturating_sub(1) {
                        black(xx, y);
                    }
                }
            };
            match shape {
                0 => fill(0, 2, 0, 14),
                1 => {
                    fill(0, wide, 4, 6);
                    fill(0, wide, 12, 14);
                    fill(0, 2, 4, 14);
                    fill(wide - 2, wide, 4, 14);
                }
                2 => {
                    fill(0, 2, 4, 14);
                    fill(wide - 2, wide, 6, 14);
                    fill(0, wide - 1, 4, 6);
                }
                _ => {
                    fill(0, 2, 7, 14);
                    fill(0, 2, 2, 4);
                }
            }
            x += wide + 2 + (next() % 2) as usize + if next() % 6 == 0 { 7 } else { 0 };
        }
    }
    data
}

/// A 1-bit image of `data`, Flate-compressed (with `predictor`, a PNG predictor, if any), with
/// `entries` added to its dictionary.
fn mono_image(doc: &mut Document, w: u32, h: u32, data: &[u8], predictor: Option<i64>, entries: &[(&str, Object)]) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v.clone());
    }
    let s = match predictor {
        None => Stream::flate(d, data),
        Some(predictor) => {
            let params = pdfcraft_filters::Params { predictor, colors: 1, bits_per_component: 1, columns: w as i64, early_change: 1 };
            let mut parms = Dict::new();
            parms.set(b"Predictor".to_vec(), Object::Int(predictor));
            parms.set(b"BitsPerComponent".to_vec(), Object::Int(1));
            parms.set(b"Columns".to_vec(), Object::Int(w as i64));
            d.set(b"Filter".to_vec(), Object::name("FlateDecode"));
            d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
            Stream::from_raw(d, pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, data).unwrap())
        }
    };
    doc.add(Object::Stream(s))
}

fn render(doc: &Document) -> Vec<u8> {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    out.rgba.to_vec()
}

/// Black-and-white images become CCITT Group 4, losslessly: the page looks the same to the pixel.
/// Grey, an image mask (with a decode array that inverts it), an indexed image of two colours and
/// one stored with a PNG predictor qualify; noise (bigger as G4), a 1-bit RGB image, one whose
/// colour space is a resource name, one wider than Group 4 encodes and a tiny one (bigger as
/// stored) are left alone, as is everything with `mono: Retain`.
#[test]
fn black_and_white_images_become_ccitt_group_4() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200, 480);
    let text = bilevel(w, h, false);
    let gray = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    let decode = Object::Array(vec![1.into(), 0.into()]);
    let mask = mono_image(&mut doc, w, h, &text, None, &[("ImageMask", Object::Bool(true)), ("Decode", decode.clone())]);
    let indexed_cs = Object::Array(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        1.into(),
        Object::String(pdfcraft_cos::PdfString::literal(vec![0xC0, 0x10, 0x10, 0xF8, 0xF0, 0xE0])),
    ]);
    let indexed = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", indexed_cs.clone())]);
    let predicted = mono_image(&mut doc, w, h, &text, Some(15), &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    let noise =
        mono_image(&mut doc, 400, 200, &bilevel(400, 200, true), None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    let rgb = mono_image(
        &mut doc,
        400,
        160,
        &bilevel(1200, 160, false),
        None,
        &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceRGB"))],
    );
    let wide = mono_image(
        &mut doc,
        40_000,
        2,
        &bilevel(40_000, 2, false),
        None,
        &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))],
    );
    // A bare /Indexed is no colour space on its own: it names the page's resource, here RGB, so
    // these samples are three to a pixel.
    let named =
        mono_image(&mut doc, 400, 160, &bilevel(1200, 160, false), None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("Indexed"))]);
    // 64 × 8 white: its Group 4 data is smaller than its Flate data, but not by the longer
    // dictionary Group 4 needs.
    let tiny = mono_image(&mut doc, 64, 8, &[0xFF; 64], None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    let tiny_g4 = pdfcraft_filters::encode_g4(&[0xFF; 64], 64, 8).unwrap();
    assert!(tiny_g4.len() < stream(&doc, tiny).raw.len(), "{} against {}", tiny_g4.len(), stream(&doc, tiny).raw.len());
    let p = page(
        &mut doc,
        &[("G", gray), ("M", mask), ("I", indexed), ("P", predicted), ("N", noise), ("R", rgb), ("W", wide), ("T", tiny), ("X", named)],
        "q 300 0 0 120 20 660 cm /G Do Q q 0 0.4 0.8 rg 300 0 0 120 320 660 cm /M Do Q q 300 0 0 120 20 520 cm /I Do Q \
         q 300 0 0 120 320 520 cm /P Do Q q 200 0 0 100 20 400 cm /N Do Q q 200 0 0 80 240 400 cm /R Do Q q 560 0 0 4 20 380 cm /W Do Q q 64 0 0 8 460 400 cm /T Do Q q 200 0 0 80 240 300 cm /X Do Q",
    );
    doc.update_dict(p, |d| {
        if let Some(Object::Dict(res)) = d.get_mut(b"Resources") {
            let mut cs = Dict::new();
            cs.set(b"Indexed".to_vec(), Object::name("DeviceRGB"));
            res.set(b"ColorSpace".to_vec(), Object::Dict(cs));
        }
    })
    .unwrap();
    let before = render(&doc);
    let raw = |doc: &Document, r: ObjRef| stream(doc, r).raw;
    let untouched = [noise, rgb, wide, tiny, named].map(|r| raw(&doc, r));

    let mut kept = doc.clone();
    let r = optimize(&mut kept, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
    assert_eq!(r.images_recompressed, 0, "Retain keeps every image as it is");
    assert!([gray, mask, indexed, predicted].iter().all(|i| raw(&kept, *i) == raw(&doc, *i)));

    let r = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(r.images_recompressed, 4);
    assert!(r.image_bytes_after < r.image_bytes_before, "{} → {}", r.image_bytes_before, r.image_bytes_after);
    for i in [gray, mask, indexed, predicted] {
        let s = stream(&doc, i);
        assert_eq!(s.dict.name(b"Filter"), Some(&b"CCITTFaxDecode"[..]));
        let parms = s.dict.get(b"DecodeParms").and_then(Object::as_dict).cloned().unwrap();
        assert_eq!((parms.int(b"K"), parms.int(b"Columns"), parms.int(b"Rows")), (Some(-1), Some(w as i64), Some(h as i64)));
    }
    assert_eq!(stream(&doc, mask).dict.get(b"Decode"), Some(&decode), "the decode array stays");
    assert_eq!(stream(&doc, indexed).dict.get(b"ColorSpace"), Some(&indexed_cs), "the colour space stays");
    assert_eq!([noise, rgb, wide, tiny, named].map(|r| raw(&doc, r)), untouched);
    assert!(render(&doc) == before, "the page looks the same");
}

/// Which 1-bit images qualify for Group 4: those of one colour component, an ICC profile of one
/// component and a DeviceN of one colourant among them, but not one of three components or two
/// colourants (more than a bit a pixel); image masks of one bit, as `/BitsPerComponent 1` says or
/// by default, but not one that says 8; and images of up to 80 million pixels, not one row more.
/// The byte totals count exactly the images stored again.
#[test]
fn group_4_takes_images_of_one_bit_a_pixel_up_to_the_pixel_limit() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let icc = |doc: &mut Document, n: i64| {
        let mut d = Dict::new();
        d.set(b"N".to_vec(), Object::Int(n));
        Object::Array(vec![Object::name("ICCBased"), Object::Ref(doc.add(Object::Stream(Stream::from_raw(d, Vec::new()))))])
    };
    let device_n = |colourants: &[&str]| {
        let mut tint = Dict::new();
        tint.set(b"FunctionType".to_vec(), Object::Int(2));
        tint.set(b"Domain".to_vec(), Object::Array(vec![0.into(), 1.into()]));
        tint.set(b"N".to_vec(), Object::Int(1));
        Object::Array(vec![
            Object::name("DeviceN"),
            Object::Array(colourants.iter().map(|c| Object::name(c)).collect()),
            Object::name("DeviceGray"),
            Object::Dict(tint),
        ])
    };
    let one_bit = |cs: Object| [("BitsPerComponent", 1.into()), ("ColorSpace", cs)];
    let icc_1 = icc(&mut doc, 1);
    let icc_1 = mono_image(&mut doc, w, h, &text, None, &one_bit(icc_1));
    let icc_3 = icc(&mut doc, 3);
    let icc_3 = mono_image(&mut doc, w, h, &bilevel(3 * w, h, false), None, &one_bit(icc_3));
    let spot = mono_image(&mut doc, w, h, &text, None, &one_bit(device_n(&["Spot"])));
    let two_spots = mono_image(&mut doc, w, h, &bilevel(2 * w, h, false), None, &one_bit(device_n(&["Spot", "Other"])));
    let mask = |bits: i64| [("ImageMask", Object::Bool(true)), ("BitsPerComponent", bits.into())];
    let mask_1 = mono_image(&mut doc, w, h, &text, None, &mask(1));
    let mask_8 = mono_image(&mut doc, w, h, &bilevel(8 * w, h, false), None, &mask(8));
    // 10,000 × 8,000 is the limit itself.
    let grey = one_bit(Object::name("DeviceGray"));
    let at_limit = mono_image(&mut doc, 10_000, 8_000, &bilevel(10_000, 8_000, false), None, &grey);
    let past_limit = mono_image(&mut doc, 10_000, 8_001, &bilevel(10_000, 8_001, false), None, &grey);
    let all = [icc_1, icc_3, spot, two_spots, mask_1, mask_8, at_limit, past_limit];
    let names: Vec<String> = (0..all.len()).map(|i| format!("Im{i}")).collect();
    // 200 × 80 pt is 432 ppi, 1800 × 1440 pt 400 ppi: below where monochrome downsampling starts.
    let content: String = (0..all.len())
        .map(|i| {
            if i < 6 {
                format!("q 200 0 0 80 {} {} cm /Im{i} Do Q ", 6 + 200 * (i % 3), 690 - 100 * (i / 3))
            } else {
                format!("q 1800 0 0 1440 0 0 cm /Im{i} Do Q ")
            }
        })
        .collect();
    page(&mut doc, &names.iter().map(String::as_str).zip(all).collect::<Vec<_>>(), &content);
    let converted = [icc_1, spot, mask_1, at_limit];
    let left = [icc_3, two_spots, mask_8, past_limit];
    let before = |doc: &Document, refs: &[ObjRef]| refs.iter().map(|r| stream(doc, *r).raw).collect::<Vec<_>>();
    let (old, untouched) = (before(&doc, &converted), before(&doc, &left));
    let r = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(r.images_recompressed, converted.len(), "{r:?}");
    for i in converted {
        assert_eq!(stream(&doc, i).dict.name(b"Filter"), Some(&b"CCITTFaxDecode"[..]), "{i:?}");
    }
    assert_eq!(before(&doc, &left), untouched);
    let new = before(&doc, &converted);
    assert_eq!((r.image_bytes_before, r.image_bytes_after), (old.iter().map(|b| b.len()).sum(), new.iter().map(|b| b.len()).sum()));
}

/// An image whose /DecodeParms are a reference, or hold numbers given by reference
/// (`/Predictor 9 0 R`, which `Dict::int` doesn't follow), is read through them, as one with them
/// direct: a predictor's tag bytes would otherwise shift every row, and the image would be stored
/// again wrong. Here a 1-bit image with a PNG predictor (stored again as Group 4), an eight-bit
/// grey one with a PNG predictor at 600 ppi (resampled), an eight-bit one of only black and white
/// with PNG predictor 10 (each row's tag a 0, so the shifted rows would still be black and white;
/// packed to one bit), and a two-entry indexed one the same way come out byte for byte as with
/// direct parameters.
#[test]
fn images_whose_decode_parms_are_references_come_out_as_with_them_direct() {
    let build = |how: &str| {
        let mut doc = Document::new_empty();
        let (w, h) = (1200u32, 480u32);
        let grey = [("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))];
        let one_bit = mono_image(&mut doc, w, h, &bilevel(w, h, false), Some(15), &grey);
        let black_and_white_samples = eight_bit(&bilevel(w, h, false), w, h);
        let shades: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8 ^ ((i / w) as u8 & 0x0F)).collect();
        let params = pdfcraft_filters::Params { predictor: 15, colors: 1, bits_per_component: 8, columns: w as i64, early_change: 1 };
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), Object::Int(15));
        parms.set(b"Columns".to_vec(), Object::Int(w as i64));
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w as i64));
        d.set(b"Height".to_vec(), Object::Int(h as i64));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        d.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        let coded = pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, &shades).unwrap();
        let eight_bit = doc.add(Object::Stream(Stream::from_raw(d.clone(), coded)));
        let params = pdfcraft_filters::Params { predictor: 10, ..params };
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), Object::Int(10));
        parms.set(b"Columns".to_vec(), Object::Int(w as i64));
        d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        let black_and_white = pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, &black_and_white_samples).unwrap();
        let packed = doc.add(Object::Stream(Stream::from_raw(d.clone(), black_and_white)));
        let palette = vec![0, 0, 255, 255, 255, 255];
        d.set(
            b"ColorSpace".to_vec(),
            Object::Array(vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                1.into(),
                Object::String(pdfcraft_cos::PdfString::literal(palette)),
            ]),
        );
        let entries = samples_of(&bilevel(w, h, false), w, h, [&[0], &[1]], 8);
        let entries = pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, &entries).unwrap();
        let indexed = doc.add(Object::Stream(Stream::from_raw(d, entries)));
        for r in [one_bit, eight_bit, packed, indexed] {
            let mut s = stream(&doc, r);
            let Some(Object::Dict(mut parms)) = s.dict.get(b"DecodeParms").cloned() else { panic!("{:?}", s.dict) };
            let made = match how {
                "whole" => Object::Ref(doc.add(Object::Dict(parms))),
                "numbers" => {
                    for k in [&b"Predictor"[..], b"Columns"] {
                        let v = parms.get(k).cloned().unwrap();
                        parms.set(k.to_vec(), Object::Ref(doc.add(v)));
                    }
                    Object::Dict(parms)
                }
                _ => continue,
            };
            s.dict.set(b"DecodeParms".to_vec(), made);
            doc.set(r, Object::Stream(s));
        }
        // 300 × 120 pt is 288 ppi; 144 × 57.6 pt is 600 ppi.
        page(
            &mut doc,
            &[("A", one_bit), ("B", eight_bit), ("C", packed), ("D", indexed)],
            "q 300 0 0 120 20 600 cm /A Do Q q 144 0 0 57.6 20 400 cm /B Do Q q 300 0 0 120 20 200 cm /C Do Q \
             q 280 0 0 112 325 200 cm /D Do Q",
        );
        let before = [one_bit, eight_bit, packed, indexed].map(|r| stream(&doc, r).raw);
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!((r.images_resampled, r.images_recompressed), (1, 3), "{r:?}");
        let after = [one_bit, eight_bit, packed, indexed].map(|r| stream(&doc, r));
        assert!(before.iter().zip(&after).all(|(b, a)| *b != a.raw), "all stored again");
        assert_eq!(after[0].dict.name(b"Filter"), Some(&b"CCITTFaxDecode"[..]));
        for s in &after[2..] {
            assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.name(b"Filter")), (Some(1), Some(&b"CCITTFaxDecode"[..])));
        }
        after.map(|s| s.raw)
    };
    let direct = build("direct");
    assert_eq!(build("whole"), direct, "the dictionary by reference");
    assert_eq!(build("numbers"), direct, "the numbers inside by reference");
}

/// An image whose data decodes far past what its samples can need (a decompression bomb) is left
/// alone rather than decoded up to the 1 GiB limit: a 1-bit one that would become Group 4, an
/// eight-bit one that would be resampled, and an eight-bit one of only black and white that would
/// be packed to one bit, each with four megabytes of zeros after its samples. Stored as ASCIIHex
/// text under Flate, broken into lines of 64 digits, their samples aren't a bomb.
#[test]
fn images_whose_data_decodes_far_past_their_samples_are_left_alone() {
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let shades: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8 ^ ((i / w) as u8 & 0x0F)).collect();
    for padding in [0, 4 << 20] {
        let mut doc = Document::new_empty();
        let padded = |data: &[u8]| data.iter().copied().chain(std::iter::repeat_n(0, padding)).collect::<Vec<u8>>();
        let grey = |bits: i64| [("BitsPerComponent", bits.into()), ("ColorSpace", Object::name("DeviceGray"))];
        let one_bit = mono_image(&mut doc, w, h, &padded(&text), None, &grey(1));
        let eight_bit = mono_image(&mut doc, w, h, &padded(&shades), None, &grey(8));
        let packed = mono_image(&mut doc, w, h, &padded(&self::eight_bit(&text, w, h)), None, &grey(8));
        // 300 × 120 pt is 288 ppi; 144 × 57.6 pt is 600 ppi.
        page(
            &mut doc,
            &[("A", one_bit), ("B", eight_bit), ("C", packed)],
            "q 300 0 0 120 20 600 cm /A Do Q q 144 0 0 57.6 20 400 cm /B Do Q q 300 0 0 120 20 200 cm /C Do Q",
        );
        let before = [one_bit, eight_bit, packed].map(|r| stream(&doc, r).raw);
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        let after = [one_bit, eight_bit, packed].map(|r| stream(&doc, r).raw);
        if padding == 0 {
            assert_eq!((r.images_resampled, r.images_recompressed), (1, 2), "{r:?}");
        } else {
            assert_eq!((r.images_resampled, r.images_recompressed), (0, 0), "{r:?}");
            assert_eq!(after, before);
        }
    }
    let hex_lines = |data: &[u8]| {
        let digits: Vec<u8> = data.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
        let mut text: Vec<u8> = digits.chunks(64).flat_map(|line| line.iter().copied().chain(*b"\n")).collect();
        text.push(b'>');
        pdfcraft_filters::encode_flate(&text)
    };
    let mut doc = Document::new_empty();
    let as_hex = |doc: &mut Document, data: &[u8], bits: i64, (w, h): (u32, u32)| {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(bits));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode"), Object::name("ASCIIHexDecode")]));
        doc.add(Object::Stream(Stream::from_raw(d, hex_lines(data))))
    };
    // Large enough that its hex text in lines is more than twice its samples and a byte a row
    // plus 4 KiB: 2560 × 1200, drawn at 400 ppi.
    let page_scan = bilevel(2560, 1200, false);
    let one_bit = as_hex(&mut doc, &page_scan, 1, (2560, 1200));
    let eight_bit = as_hex(&mut doc, &shades, 8, (w, h));
    let packed = as_hex(&mut doc, &self::eight_bit(&text, w, h), 8, (w, h));
    page(
        &mut doc,
        &[("A", one_bit), ("B", eight_bit), ("C", packed)],
        "q 460.8 0 0 216 20 500 cm /A Do Q q 144 0 0 57.6 20 400 cm /B Do Q q 300 0 0 120 20 200 cm /C Do Q",
    );
    optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(stream(&doc, one_bit).dict.name(b"Filter"), Some(&b"CCITTFaxDecode"[..]), "1-bit hex text in lines: Group 4");
    assert!(stream(&doc, eight_bit).dict.int(b"Width").is_some_and(|nw| nw < i64::from(w)), "8-bit hex text in lines: resampled");
    assert_eq!(stream(&doc, packed).dict.int(b"BitsPerComponent"), Some(1), "8-bit black-and-white hex text in lines: packed");
}

/// Monochrome Images keeping each image's compression (and resampling as by default).
const RETAIN: MonoSettings = MonoSettings { compression: MonoCompression::Retain, ..MonoSettings::REDUCE };

/// Eight-bit samples (0 or 255) from rows of one-bit ones.
fn eight_bit(bits: &[u8], w: u32, h: u32) -> Vec<u8> {
    let stride = (w as usize).div_ceil(8);
    (0..h as usize).flat_map(|y| (0..w as usize).map(move |x| if bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0 { 255 } else { 0 })).collect()
}

/// Eight-bit samples of only 0 and 255, dithered (Floyd–Steinberg) from a smooth picture as a
/// scanner's halftone mode gives them: no shade but black and white, in short runs.
fn dithered(w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut level: Vec<f32> = (0..w * h).map(|i| 127.5 + 127.5 * ((i % w) as f32 / 37.0).sin() * ((i / w) as f32 / 53.0).cos()).collect();
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let v = if level[i] < 128.0 { 0.0 } else { 255.0 };
            out[i] = v as u8;
            let e = level[i] - v;
            if x + 1 < w {
                level[i + 1] += e * 7.0 / 16.0;
            }
            if y + 1 < h {
                if x > 0 {
                    level[i + w - 1] += e * 3.0 / 16.0;
                }
                level[i + w] += e * 5.0 / 16.0;
                if x + 1 < w {
                    level[i + w + 1] += e / 16.0;
                }
            }
        }
    }
    out
}

/// A dithered scan stored as eight-bit grey of only black and white is smaller downsampled and
/// JPEG-compressed by the grey settings than packed to one bit, which can't shrink its dots: the
/// smaller wins, so Reduce File Size makes it no larger than with Monochrome Images on Retain.
/// Clean text is still packed to one bit at the same resolution.
#[test]
fn eight_bit_black_and_white_takes_the_smaller_of_one_bit_and_the_grey_settings() {
    let (w, h) = (1200u32, 1500u32);
    for (what, samples) in [("dither", dithered(w, h)), ("text", eight_bit(&bilevel(w, h, false), w, h))] {
        let build = || {
            let mut doc = Document::new_empty();
            let img = mono_image(&mut doc, w, h, &samples, None, &[("BitsPerComponent", 8.into()), ("ColorSpace", Object::name("DeviceGray"))]);
            // 288 × 360 pt is 300 ppi, above where the grey settings downsample.
            page(&mut doc, &[("A", img)], "q 288 0 0 360 20 400 cm /A Do Q");
            (doc, img)
        };
        let (mut doc, img) = build();
        optimize(&mut doc, &Settings::default()).unwrap();
        let (mut retained, _) = build();
        optimize(&mut retained, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
        let (s, kept) = (stream(&doc, img), stream(&retained, img));
        let bits = if what == "dither" { 8 } else { 1 };
        assert_eq!(s.dict.int(b"BitsPerComponent"), Some(bits), "{what}: {} bytes, {} with Retain", s.raw.len(), kept.raw.len());
        if what == "dither" {
            assert!(s.raw.len() <= kept.raw.len(), "{what}: {} bytes, {} with Retain", s.raw.len(), kept.raw.len());
        }
    }
}

/// The smaller of one bit and the grey settings counts the soft mask each leaves: packed, the
/// image keeps its mask as it is; resampled by the grey settings, the mask is resampled with it.
/// Clean text, 1200 × 1500 at 300 ppi, under a noisy mask. A mask as large: the grey settings
/// shrink it by far more than packing saves on the image, and win. A mask of 640 × 800 (a soft
/// mask may be another size than its image): resampled to the image's new size it shrinks only a
/// little, by less than the grey settings' JPEG adds over the packed image, and packing wins.
#[test]
fn eight_bit_black_and_white_with_a_soft_mask_counts_the_mask() {
    let (w, h) = (1200u32, 1500u32);
    for ((mw, mh), grey_wins) in [((w, h), true), ((640, 800), false)] {
        let mut doc = Document::new_empty();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let noise: Vec<u8> = (0..mw * mh)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect();
        let mask = mono_image(&mut doc, mw, mh, &noise, None, &[("BitsPerComponent", 8.into()), ("ColorSpace", Object::name("DeviceGray"))]);
        let grey = [("BitsPerComponent", 8.into()), ("ColorSpace", Object::name("DeviceGray")), ("SMask", Object::Ref(mask))];
        let img = mono_image(&mut doc, w, h, &eight_bit(&bilevel(w, h, false), w, h), None, &grey);
        page(&mut doc, &[("A", img)], "q 288 0 0 360 20 400 cm /A Do Q");
        optimize(&mut doc, &Settings::default()).unwrap();
        let s = stream(&doc, img);
        let expected = if grey_wins { (Some(8), Some(&b"DCTDecode"[..])) } else { (Some(1), Some(&b"CCITTFaxDecode"[..])) };
        assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.name(b"Filter")), expected, "mask {mw} × {mh}: {:?}", s.dict);
    }
}

/// Eight-bit grey images of only black and white (as some scanners and converters write scans)
/// become one-bit Group 4, losslessly: the page looks the same, an inverting decode array
/// included. Not touched by this: a grey image with any other shade, an indexed image (its samples
/// are palette indices), one masked by colour key, and everything with `mono: Retain`.
#[test]
fn eight_bit_black_and_white_images_become_one_bit() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200, 480);
    let grey = eight_bit(&bilevel(w, h, false), w, h);
    let image = |doc: &mut Document, data: &[u8], entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w as i64));
        d.set(b"Height".to_vec(), Object::Int(h as i64));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        doc.add(Object::Stream(Stream::flate(d, data)))
    };
    let gray_cs = ("ColorSpace", Object::name("DeviceGray"));
    let plain = image(&mut doc, &grey, std::slice::from_ref(&gray_cs));
    let decode = Object::Array(vec![1.into(), 0.into()]);
    let inverted = image(&mut doc, &grey, &[gray_cs.clone(), ("Decode", decode.clone())]);
    let mut shaded = grey.clone();
    shaded[1000] = 128;
    let shade = image(&mut doc, &shaded, std::slice::from_ref(&gray_cs));
    let palette = Object::Array(vec![
        Object::name("Indexed"),
        Object::name("DeviceGray"),
        255.into(),
        Object::String(pdfcraft_cos::PdfString::literal((0..=255u8).rev().collect::<Vec<_>>())),
    ]);
    let indexed = image(&mut doc, &grey, &[("ColorSpace", palette)]);
    let keyed = image(&mut doc, &grey, &[gray_cs, ("Mask", Object::Array(vec![0.into(), 0.into()]))]);
    page(
        &mut doc,
        &[("A", plain), ("B", inverted), ("C", shade), ("D", indexed), ("E", keyed)],
        "q 300 0 0 120 20 660 cm /A Do Q q 300 0 0 120 320 660 cm /B Do Q q 300 0 0 120 20 520 cm /C Do Q \
         q 300 0 0 120 320 520 cm /D Do Q q 300 0 0 120 20 380 cm /E Do Q",
    );
    let before = render(&doc);
    let raw = |doc: &Document, r: ObjRef| stream(doc, r).raw;
    // Grey settings that leave grey images alone, so only this conversion acts.
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let settings = Settings { gray: keep, ..Settings::default() };

    let mut kept = doc.clone();
    let r = optimize(&mut kept, &Settings { mono: RETAIN, ..settings.clone() }).unwrap();
    assert_eq!(r.images_recompressed, 0, "Retain keeps every image as it is");

    let untouched = [shade, indexed, keyed].map(|r| raw(&doc, r));
    let old: usize = [plain, inverted].map(|r| raw(&doc, r).len()).iter().sum();
    let r = optimize(&mut doc, &settings).unwrap();
    assert_eq!(r.images_recompressed, 2);
    let new: usize = [plain, inverted].map(|r| raw(&doc, r).len()).iter().sum();
    assert_eq!((r.image_bytes_before, r.image_bytes_after), (old, new), "the byte totals count both");
    for i in [plain, inverted] {
        let s = stream(&doc, i);
        assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.name(b"Filter")), (Some(1), Some(&b"CCITTFaxDecode"[..])));
    }
    assert_eq!(stream(&doc, inverted).dict.get(b"Decode"), Some(&decode), "the decode array stays");
    assert_eq!([shade, indexed, keyed].map(|r| raw(&doc, r)), untouched);
    assert!(render(&doc) == before, "the page looks the same");
}

/// `bits` (1-bit rows) with each 0 as `values[0]` and each 1 as `values[1]`, at `bpc` bits a sample
/// (each row starting on a new byte).
fn samples_of(bits: &[u8], w: u32, h: u32, values: [&[u8]; 2], bpc: usize) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let stride = w.div_ceil(8);
    let mut out = Vec::new();
    for y in 0..h {
        let row: Vec<u8> = (0..w).flat_map(|x| values[usize::from(bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0)].iter().copied()).collect();
        let mut packed = vec![0u8; (row.len() * bpc).div_ceil(8)];
        for (i, s) in row.iter().enumerate() {
            packed[i * bpc / 8] |= if bpc == 8 { *s } else { s << (8 - bpc - i * bpc % 8) };
        }
        out.extend(packed);
    }
    out
}

/// Eight-bit RGB of only black and white becomes one-bit grey, and an indexed image that uses only
/// palette entries 0 and 1 becomes one bit with its palette (here blue and white), at 8 bits or 4,
/// both as Group 4 and losslessly: the page looks the same to the pixel. An RGB decode array the
/// same for each component becomes grey's, and an indexed image's default one goes. Not touched:
/// RGB with any other colour, a decode array that differs between components, a colour-key mask or
/// calibrated RGB; an indexed image that uses a third entry or has another decode array; and
/// everything with `mono: Retain`.
#[test]
fn rgb_and_two_colour_indexed_black_and_white_images_become_one_bit() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let image = |doc: &mut Document, data: &[u8], bpc: i64, cs: Object, entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
        d.set(b"ColorSpace".to_vec(), cs);
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        doc.add(Object::Stream(Stream::flate(d, data)))
    };
    let rgb_cs = Object::name("DeviceRGB");
    let rgb_data = samples_of(&text, w, h, [&[0, 0, 0], &[255, 255, 255]], 8);
    let rgb = image(&mut doc, &rgb_data, 8, rgb_cs.clone(), &[]);
    let pairs = |p: [i64; 6]| Object::Array(p.into_iter().map(Object::Int).collect());
    let rgb_inverted = image(&mut doc, &rgb_data, 8, rgb_cs.clone(), &[("Decode", pairs([1, 0, 1, 0, 1, 0]))]);
    let rgb_mixed = image(&mut doc, &rgb_data, 8, rgb_cs.clone(), &[("Decode", pairs([1, 0, 0, 1, 0, 1]))]);
    let mut red = rgb_data.clone();
    red[3000..3003].copy_from_slice(&[255, 0, 0]);
    let rgb_red = image(&mut doc, &red, 8, rgb_cs.clone(), &[]);
    let mut white_point = Dict::new();
    white_point.set(b"WhitePoint".to_vec(), Object::Array(vec![Object::Real(0.9505), 1.into(), Object::Real(1.089)]));
    let cal_rgb = image(&mut doc, &rgb_data, 8, Object::Array(vec![Object::name("CalRGB"), Object::Dict(white_point)]), &[]);
    let rgb_keyed = image(&mut doc, &rgb_data, 8, rgb_cs, &[("Mask", pairs([0, 0, 0, 0, 0, 0]))]);
    let palette = |entries: &[u8]| {
        Object::Array(vec![
            Object::name("Indexed"),
            Object::name("DeviceRGB"),
            (entries.len() as i64 / 3 - 1).into(),
            Object::String(pdfcraft_cos::PdfString::literal(entries.to_vec())),
        ])
    };
    let blue_white = palette(&[0, 0, 255, 255, 255, 255]);
    let indices = samples_of(&text, w, h, [&[0], &[1]], 8);
    let blue = image(&mut doc, &indices, 8, blue_white.clone(), &[]);
    let default_decode = Object::Array(vec![0.into(), 255.into()]);
    let blue_decoded = image(&mut doc, &indices, 8, blue_white.clone(), &[("Decode", default_decode)]);
    let blue_4 = image(&mut doc, &samples_of(&text, w, h, [&[0], &[1]], 4), 4, blue_white.clone(), &[]);
    let mut third = indices.clone();
    third[3000] = 2;
    let three = image(&mut doc, &third, 8, palette(&[0, 0, 255, 255, 255, 255, 255, 0, 0]), &[]);
    let other_decode = image(&mut doc, &indices, 8, blue_white.clone(), &[("Decode", Object::Array(vec![0.into(), 1.into()]))]);
    let all = [rgb, rgb_inverted, rgb_mixed, rgb_red, cal_rgb, rgb_keyed, blue, blue_decoded, blue_4, three, other_decode];
    // 200 × 80 pt is 432 ppi: below where monochrome downsampling starts.
    let names: Vec<String> = (0..all.len()).map(|i| format!("Im{i}")).collect();
    let content: String =
        (0..all.len()).map(|i| format!("q 200 0 0 80 {} {} cm /Im{i} Do Q ", 6 + 200 * (i % 3), 690 - 100 * (i / 3) as i64)).collect();
    page(&mut doc, &names.iter().map(String::as_str).zip(all).collect::<Vec<_>>(), &content);
    let before = render(&doc);
    let raw = |doc: &Document, r: ObjRef| stream(doc, r).raw;
    // Colour and grey settings that leave images alone, so only this conversion acts.
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let settings = Settings { color: keep, gray: keep, ..Settings::default() };

    let mut kept = doc.clone();
    let r = optimize(&mut kept, &Settings { mono: RETAIN, ..settings.clone() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 0), "Retain keeps every image as it is");

    let untouched = [rgb_mixed, rgb_red, cal_rgb, rgb_keyed, three, other_decode].map(|r| raw(&doc, r));
    let r = optimize(&mut doc, &settings).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 5), "{r:?}");
    for (i, cs) in [
        (rgb, Object::name("DeviceGray")),
        (rgb_inverted, Object::name("DeviceGray")),
        (blue, blue_white.clone()),
        (blue_decoded, blue_white.clone()),
        (blue_4, blue_white),
    ] {
        let s = stream(&doc, i);
        assert_eq!(
            (s.dict.int(b"BitsPerComponent"), s.dict.name(b"Filter"), s.dict.get(b"ColorSpace")),
            (Some(1), Some(&b"CCITTFaxDecode"[..]), Some(&cs)),
            "{i:?}"
        );
        assert_eq!(doc.decoded_image(&s).unwrap(), text, "{i:?}");
        let decode = if i == rgb_inverted { Some(Object::Array(vec![1.into(), 0.into()])) } else { None };
        assert_eq!(s.dict.get(b"Decode").cloned(), decode, "{i:?}");
    }
    assert_eq!([rgb_mixed, rgb_red, cal_rgb, rgb_keyed, three, other_decode].map(|r| raw(&doc, r)), untouched);
    assert!(render(&doc) == before, "the page looks the same");
}

/// Eight-bit black-and-white images in other one-component spaces are packed too: calibrated
/// grey, an ICC profile of one component, a separation, each keeping its colour space. Calibrated
/// RGB whose samples are only 0 and 1 is no indexed image, and stays as it is.
#[test]
fn eight_bit_black_and_white_in_other_one_component_spaces_is_packed() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let image = |doc: &mut Document, data: &[u8], cs: Object| {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), cs);
        doc.add(Object::Stream(Stream::flate(d, data)))
    };
    let mut white_point = Dict::new();
    white_point.set(b"WhitePoint".to_vec(), Object::Array(vec![Object::Real(0.9505), 1.into(), Object::Real(1.089)]));
    let mut profile = Dict::new();
    profile.set(b"N".to_vec(), Object::Int(1));
    let profile = doc.add(Object::Stream(Stream::from_raw(profile, Vec::new())));
    let mut tint = Dict::new();
    tint.set(b"FunctionType".to_vec(), Object::Int(2));
    tint.set(b"Domain".to_vec(), Object::Array(vec![0.into(), 1.into()]));
    tint.set(b"N".to_vec(), Object::Int(1));
    let spaces = [
        Object::Array(vec![Object::name("CalGray"), Object::Dict(white_point.clone())]),
        Object::Array(vec![Object::name("ICCBased"), Object::Ref(profile)]),
        Object::Array(vec![Object::name("Separation"), Object::name("Spot"), Object::name("DeviceGray"), Object::Dict(tint)]),
    ];
    let grey = samples_of(&text, w, h, [&[0], &[255]], 8);
    let packed = spaces.clone().map(|cs| image(&mut doc, &grey, cs));
    let cal_rgb = Object::Array(vec![Object::name("CalRGB"), Object::Dict(white_point)]);
    let cal_rgb = image(&mut doc, &samples_of(&text, w, h, [&[0, 0, 0], &[1, 1, 1]], 8), cal_rgb);
    let names = ["A", "B", "C", "D"];
    // 200 × 80 pt is 432 ppi: below where monochrome downsampling starts.
    let content: String = (0..4).map(|i| format!("q 200 0 0 80 6 {} cm /{} Do Q ", 690 - 100 * i, names[i])).collect();
    page(&mut doc, &names.into_iter().zip([packed[0], packed[1], packed[2], cal_rgb]).collect::<Vec<_>>(), &content);
    let untouched = stream(&doc, cal_rgb).raw;
    // Colour and grey settings that leave images alone, so only the packing acts.
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    optimize(&mut doc, &Settings { color: keep, gray: keep, ..Settings::default() }).unwrap();
    for (r, cs) in packed.iter().zip(&spaces) {
        let s = stream(&doc, *r);
        assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.get(b"ColorSpace")), (Some(1), Some(cs)), "{cs:?}");
    }
    assert_eq!(stream(&doc, cal_rgb).raw, untouched, "calibrated RGB");
}

/// A resampled Group 4 image is stored as 1-bit Flate when that is smaller, as any resampled image
/// may be: here vertical stripes, whose identical rows Flate takes for almost nothing.
#[test]
fn a_resampled_group_4_image_is_stored_as_flate_when_smaller() {
    let (w, h) = (1200u32, 480u32);
    let stripes = vec![0x0Fu8; (w as usize).div_ceil(8) * h as usize];
    let mut doc = Document::new_empty();
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w.into()));
    d.set(b"Height".to_vec(), Object::Int(h.into()));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
    let mut parms = Dict::new();
    parms.set(b"K".to_vec(), Object::Int(-1));
    parms.set(b"Columns".to_vec(), Object::Int(w.into()));
    d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    let img = doc.add(Object::Stream(Stream::from_raw(d, pdfcraft_filters::encode_g4(&stripes, w, h).unwrap())));
    // 144 × 57.6 pt is 600 ppi.
    page(&mut doc, &[("A", img)], "q 144 0 0 57.6 20 600 cm /A Do Q");
    let r = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(r.images_resampled, 1, "{r:?}");
    let s = stream(&doc, img);
    assert_eq!((s.dict.int(b"Width"), s.dict.name(b"Filter")), (Some(600), Some(&b"FlateDecode"[..])));
}

/// A decode array is read as numbers, whichever way they are written: an indexed image's default
/// as `[0 255.0]` or with an entry by reference, and RGB's inverting pairs with a real among them,
/// pack as when written as integers.
#[test]
fn decode_arrays_read_as_numbers_however_written() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let image = |doc: &mut Document, data: &[u8], cs: Object, decode: Object| {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), cs);
        d.set(b"Decode".to_vec(), decode);
        doc.add(Object::Stream(Stream::flate(d, data)))
    };
    let palette = Object::Array(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        1.into(),
        Object::String(pdfcraft_cos::PdfString::literal(vec![0, 0, 255, 255, 255, 255])),
    ]);
    let indices = samples_of(&text, w, h, [&[0], &[1]], 8);
    let max = doc.add(Object::Int(255));
    let real = image(&mut doc, &indices, palette.clone(), Object::Array(vec![0.into(), Object::Real(255.0)]));
    let referred = image(&mut doc, &indices, palette, Object::Array(vec![0.into(), Object::Ref(max)]));
    let rgb = samples_of(&text, w, h, [&[0, 0, 0], &[255, 255, 255]], 8);
    let pairs = Object::Array(vec![1.into(), 0.into(), 1.into(), 0.into(), Object::Real(1.0), 0.into()]);
    let inverted = image(&mut doc, &rgb, Object::name("DeviceRGB"), pairs);
    page(
        &mut doc,
        &[("A", real), ("B", referred), ("C", inverted)],
        "q 200 0 0 80 6 690 cm /A Do Q q 200 0 0 80 6 590 cm /B Do Q q 200 0 0 80 6 490 cm /C Do Q",
    );
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    optimize(&mut doc, &Settings { color: keep, gray: keep, ..Settings::default() }).unwrap();
    for r in [real, referred] {
        let s = stream(&doc, r);
        assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.get(b"Decode")), (Some(1), None), "{r:?}");
    }
    let s = stream(&doc, inverted);
    let decode = Object::Array(vec![1.into(), 0.into()]);
    assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.get(b"Decode")), (Some(1), Some(&decode)));
}

/// With Retain, a Group 3 halftone above 450 ppi, which isn't resampled, stays Group 3 byte for
/// byte: Retain stores nothing again that it doesn't resample.
#[test]
fn retain_keeps_a_group_3_halftone_as_it_is() {
    let (w, h) = (64u32, 64u32);
    let a: Vec<u32> = vec![1; 64];
    let b: Vec<u32> = std::iter::once(0).chain(std::iter::repeat_n(1, 64)).collect();
    let runs: Vec<&[u32]> = (0..h).map(|y| if y % 2 == 0 { &a[..] } else { &b[..] }).collect();
    let g3 = group_3(&runs, false, None);
    let mut doc = Document::new_empty();
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w.into()));
    d.set(b"Height".to_vec(), Object::Int(h.into()));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
    let mut parms = Dict::new();
    parms.set(b"K".to_vec(), Object::Int(0));
    parms.set(b"Columns".to_vec(), Object::Int(w.into()));
    d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    let fax = doc.add(Object::Stream(Stream::from_raw(d, g3.clone())));
    // 7.68 pt for 64 samples is 600 ppi.
    page(&mut doc, &[("A", fax)], "q 7.68 0 0 7.68 100 600 cm /A Do Q");
    assert_eq!(doc.decoded_image(&stream(&doc, fax)).unwrap().len(), 64 * 8, "the fixture decodes");
    let r = optimize(&mut doc, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 0), "{r:?}");
    assert_eq!(stream(&doc, fax).raw[..], g3[..]);
}

/// Black-and-white RGB stays RGB where grey would be drawn otherwise: drawn, on any of its uses,
/// where `/DefaultGray` or `/DefaultRGB` is in force (ISO 32000-2 §8.6.5.6), from a page's
/// resources or a form's own, as grey would then go through DefaultGray, or no longer through
/// DefaultRGB; or with a soft mask whose `/Matte` has three components, one for each of RGB's. A
/// form drawn where they are in force counts too, though it has resources of its own. The same
/// image drawn plainly, or with a soft mask and no `/Matte`, becomes grey.
#[test]
fn black_and_white_rgb_stays_rgb_where_grey_would_be_drawn_otherwise() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let rgb_data = samples_of(&bilevel(w, h, false), w, h, [&[0, 0, 0], &[255, 255, 255]], 8);
    let image = |doc: &mut Document, cs: &str, data: &[u8], entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), Object::name(cs));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        doc.add(Object::Stream(Stream::flate(d, data)))
    };
    let opaque = vec![255u8; (w * h) as usize];
    let plain_mask = image(&mut doc, "DeviceGray", &opaque, &[]);
    let matte_mask = image(&mut doc, "DeviceGray", &opaque, &[("Matte", Object::Array(vec![0.into(); 3]))]);
    let plain = image(&mut doc, "DeviceRGB", &rgb_data, &[]);
    let masked = image(&mut doc, "DeviceRGB", &rgb_data, &[("SMask", Object::Ref(plain_mask))]);
    let matted = image(&mut doc, "DeviceRGB", &rgb_data, &[("SMask", Object::Ref(matte_mask))]);
    let [under_gray, under_rgb, shared, in_form, nested] = [(); 5].map(|_| image(&mut doc, "DeviceRGB", &rgb_data, &[]));
    let mut white_point = Dict::new();
    white_point.set(b"WhitePoint".to_vec(), Object::Array(vec![Object::Real(0.9505), 1.into(), Object::Real(1.089)]));
    let spaces = |key: &str| {
        let family = if key == "DefaultGray" { "CalGray" } else { "CalRGB" };
        let mut c = Dict::new();
        c.set(key.as_bytes().to_vec(), Object::Array(vec![Object::name(family), Object::Dict(white_point.clone())]));
        Object::Dict(c)
    };
    let draw = |names: &[&str]| names.iter().enumerate().map(|(i, n)| format!("q 200 0 0 80 6 {} cm /{n} Do Q ", 690 - 100 * i)).collect::<String>();
    let form = |doc: &mut Document, im: ObjRef, spaces: Option<Object>| {
        let mut xobjects = Dict::new();
        xobjects.set(b"Im".to_vec(), Object::Ref(im));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        if let Some(spaces) = spaces {
            resources.set(b"ColorSpace".to_vec(), spaces);
        }
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("XObject"));
        f.set(b"Subtype".to_vec(), Object::name("Form"));
        f.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        f.set(b"Resources".to_vec(), Object::Dict(resources));
        doc.add(Object::Stream(Stream::from_raw(f, draw(&["Im"]).into_bytes())))
    };
    // Drawn plainly first, then where DefaultGray is in force: the second walk must happen.
    let plain_form = form(&mut doc, nested, None);
    page(&mut doc, &[("A", plain), ("B", masked), ("C", matted), ("D", shared), ("E", plain_form)], &draw(&["A", "B", "C", "D", "E"]));
    for (key, xobjects) in [("DefaultGray", vec![("A", under_gray), ("B", shared), ("C", plain_form)]), ("DefaultRGB", vec![("A", under_rgb)])] {
        let names: Vec<&str> = xobjects.iter().map(|(n, _)| *n).collect();
        let p = page(&mut doc, &xobjects, &draw(&names));
        doc.update_dict(p, |d| {
            let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
            res.set(b"ColorSpace".to_vec(), spaces(key));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        })
        .unwrap();
    }
    // A form with its own resources, DefaultGray among them, on a page without any.
    let gray_form = form(&mut doc, in_form, Some(spaces("DefaultGray")));
    page(&mut doc, &[("F", gray_form)], "/F Do");
    let kept = [under_gray, under_rgb, shared, in_form, nested, matted];
    let before = kept.map(|r| stream(&doc, r));
    // Colour and grey settings that leave images alone, so only this conversion acts.
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let settings = Settings { color: keep, gray: keep, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 2), "{r:?}");
    for i in [plain, masked] {
        let s = stream(&doc, i);
        assert_eq!((s.dict.get(b"ColorSpace"), s.dict.int(b"BitsPerComponent")), (Some(&Object::name("DeviceGray")), Some(1)), "{i:?}");
    }
    for (i, b) in kept.iter().zip(&before) {
        let s = stream(&doc, *i);
        assert_eq!((&s.dict, &s.raw), (&b.dict, &b.raw), "{i:?}");
    }

    // A use that couldn't be followed (forms nested too deep) may be where DefaultGray is in force.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, "DeviceRGB", &rgb_data, &[]);
    let deep = nested_forms(&mut doc, img, 13);
    page(&mut doc, &[("Im", img), ("F", deep)], &draw(&["Im", "F"]));
    let before = stream(&doc, img);
    optimize(&mut doc, &settings).unwrap();
    assert_eq!(stream(&doc, img).raw, before.raw, "a use not followed");
}

/// An empty document, and a maker of images of only black and white as eight-bit `DeviceRGB`.
fn rgb_page_fixture() -> (Document, impl Fn(&mut Document) -> ObjRef) {
    let (w, h) = (1200u32, 480u32);
    let rgb_data = samples_of(&bilevel(w, h, false), w, h, [&[0, 0, 0], &[255, 255, 255]], 8);
    let image = move |doc: &mut Document| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        doc.add(Object::Stream(Stream::flate(d, &rgb_data)))
    };
    (Document::new_empty(), image)
}

/// `q … cm /N Do Q` for each name, one under the other.
fn draw_each(names: &[&str]) -> String {
    names.iter().enumerate().map(|(i, n)| format!("q 200 0 0 80 6 {} cm /{n} Do Q ", 690 - 100 * i)).collect()
}

/// An `/XObject` dictionary of `names`.
fn xobject_dict(names: &[(&str, ObjRef)]) -> Dict {
    let mut x = Dict::new();
    for (n, r) in names {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    x
}

/// A stream drawing `/Im` (or nothing) whose `/Resources` is `resources`, with `extra` in its
/// dictionary: a form, an appearance or a tiling pattern.
fn drawing(doc: &mut Document, resources: Option<Object>, extra: &[(&str, Object)]) -> ObjRef {
    let mut f = Dict::new();
    f.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    if let Some(resources) = resources {
        f.set(b"Resources".to_vec(), resources);
    }
    for (k, v) in extra {
        f.set(k.as_bytes().to_vec(), v.clone());
    }
    doc.add(Object::Stream(Stream::from_raw(f, draw_each(&["Im"]).into_bytes())))
}

fn form_entries() -> [(&'static str, Object); 2] {
    [("Type", Object::name("XObject")), ("Subtype", Object::name("Form"))]
}

/// Resources of `entries`.
fn resources_of(entries: Vec<(&str, Object)>) -> Dict {
    let mut res = Dict::new();
    for (k, v) in entries {
        res.set(k.as_bytes().to_vec(), v);
    }
    res
}

/// Give page `p` an annotation whose normal appearance is `appearance`.
fn annotate(doc: &mut Document, p: ObjRef, appearance: ObjRef) {
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(appearance));
    let mut annotation = Dict::new();
    annotation.set(b"Subtype".to_vec(), Object::name("Square"));
    annotation.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()]));
    annotation.set(b"AP".to_vec(), Object::Dict(ap));
    let annotation = doc.add(Object::Dict(annotation));
    let mut annots = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").and_then(|a| a.as_array().cloned())).unwrap_or_default();
    annots.push(Object::Ref(annotation));
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
}

/// `/DefaultGray` as a calibrated grey, in a `/ColorSpace` resource dictionary.
fn default_gray() -> Object {
    let mut white_point = Dict::new();
    white_point.set(b"WhitePoint".to_vec(), Object::Array(vec![Object::Real(0.9505), 1.into(), Object::Real(1.089)]));
    let mut c = Dict::new();
    c.set(b"DefaultGray".to_vec(), Object::Array(vec![Object::name("CalGray"), Object::Dict(white_point)]));
    Object::Dict(c)
}

/// Optimize with colour and grey settings that leave images alone, so only packing acts; the
/// report's images resampled and stored again.
fn pack_only(doc: &mut Document) -> (usize, usize) {
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let r = optimize(doc, &Settings { color: keep, gray: keep, ..Settings::default() }).unwrap();
    (r.images_resampled, r.images_recompressed)
}

/// Black-and-white RGB becomes grey for every use at once, so it stays RGB, byte for byte, when
/// any use may be beyond the page content and forms the optimizer reads, where other default
/// colour spaces may be in force: drawn by an annotation's appearance whose resources make
/// `/DefaultGray` the grey it would be drawn in; by a tiling pattern; through an `/XObject`
/// dictionary, or a form, that a page shares with such an appearance;
/// referred to with another generation by the page than by such an appearance; drawn plainly on
/// one page and, named with another generation, on a page whose resources set `/DefaultGray`;
/// drawn plainly and only listed (not drawn) where `/DefaultGray` is in force, under the same
/// generation or another, as a Type3 glyph without resources could draw it there; or held in an
/// array elsewhere (another image's `/Alternates`).
#[test]
fn black_and_white_rgb_used_beyond_the_page_content_read_stays_rgb() {
    let (mut doc, image) = rgb_page_fixture();
    let [in_annotation, in_pattern, shared_xobjects, shared_form, other_generation, generation_under_defaults, listed, listed_as_1, in_array] =
        [(); 9].map(|_| image(&mut doc));
    let gray_resources = |im: Object| Some(Object::Dict(resources_of(vec![("XObject", im), ("ColorSpace", default_gray())])));
    let direct = |im: ObjRef| Object::Dict(xobject_dict(&[("Im", im)]));

    let appearance = drawing(&mut doc, gray_resources(direct(in_annotation)), &form_entries());
    let pattern_entries =
        [("PatternType", 1.into()), ("PaintType", 1.into()), ("TilingType", 1.into()), ("XStep", 612.into()), ("YStep", 792.into())];
    let pattern = drawing(&mut doc, Some(Object::Dict(resources_of(vec![("XObject", direct(in_pattern))]))), &pattern_entries);
    let p = page(&mut doc, &[("A", in_annotation), ("B", in_pattern)], &(draw_each(&["A", "B"]) + "/Pattern cs /P scn 0 0 10 10 re f"));
    annotate(&mut doc, p, appearance);
    doc.update_dict(p, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut patterns = Dict::new();
        patterns.set(b"P".to_vec(), Object::Ref(pattern));
        res.set(b"Pattern".to_vec(), Object::Dict(patterns));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();

    // An /XObject dictionary of its own, read for a page, and a form drawn by a page, each also
    // used by an appearance under DefaultGray.
    let x = doc.add(Object::Dict(xobject_dict(&[("Im", shared_xobjects)])));
    let q = page(&mut doc, &[], &draw_each(&["Im"]));
    doc.update_dict(q, |d| d.set(b"Resources".to_vec(), Object::Dict(resources_of(vec![("XObject", Object::Ref(x))])))).unwrap();
    let appearance = drawing(&mut doc, gray_resources(Object::Ref(x)), &form_entries());
    annotate(&mut doc, q, appearance);
    let form = drawing(&mut doc, Some(Object::Dict(resources_of(vec![("XObject", direct(shared_form))]))), &form_entries());
    let q = page(&mut doc, &[("F", form)], &draw_each(&["F"]));
    let appearance = drawing(&mut doc, gray_resources(Object::Dict(xobject_dict(&[("Im", form)]))), &form_entries());
    annotate(&mut doc, q, appearance);

    // The page names it with generation 1 (it is 0); the appearance with 0.
    let q = page(&mut doc, &[("Im", ObjRef { generation: 1, ..other_generation })], &draw_each(&["Im"]));
    let appearance = drawing(&mut doc, gray_resources(direct(other_generation)), &form_entries());
    annotate(&mut doc, q, appearance);

    // Drawn plainly as 0, and as 1 where DefaultGray is in force.
    page(&mut doc, &[("Im", generation_under_defaults)], &draw_each(&["Im"]));
    let q = page(&mut doc, &[], &draw_each(&["Im"]));
    let gray = resources_of(vec![
        ("XObject", Object::Dict(xobject_dict(&[("Im", ObjRef { generation: 1, ..generation_under_defaults })]))),
        ("ColorSpace", default_gray()),
    ]);
    doc.update_dict(q, |d| d.set(b"Resources".to_vec(), Object::Dict(gray))).unwrap();

    // Drawn plainly, and only listed where DefaultGray is in force, as 0 and as 1.
    page(&mut doc, &[("A", listed), ("B", listed_as_1)], &draw_each(&["A", "B"]));
    let q = page(&mut doc, &[], "");
    let gray = resources_of(vec![
        ("XObject", Object::Dict(xobject_dict(&[("A", listed), ("B", ObjRef { generation: 1, ..listed_as_1 })]))),
        ("ColorSpace", default_gray()),
    ]);
    doc.update_dict(q, |d| d.set(b"Resources".to_vec(), Object::Dict(gray))).unwrap();

    // Another image drawn on the page lists it as an alternate.
    let mut alternate = Dict::new();
    alternate.set(b"Image".to_vec(), Object::Ref(in_array));
    let holder = image(&mut doc);
    doc.update_dict(holder, |d| {
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        d.set(b"Alternates".to_vec(), Object::Array(vec![Object::Dict(alternate)]));
    })
    .unwrap();
    page(&mut doc, &[("Im", in_array), ("Alt", holder)], &draw_each(&["Im"]));

    let kept = [in_annotation, in_pattern, shared_xobjects, shared_form, other_generation, generation_under_defaults, listed, listed_as_1, in_array];
    let before = kept.map(|r| stream(&doc, r));
    pack_only(&mut doc);
    for (i, b) in kept.iter().zip(&before) {
        let s = stream(&doc, *i);
        assert_eq!((&s.dict, &s.raw), (&b.dict, &b.raw), "{i:?}");
    }
}

/// Black-and-white RGB that the page content and forms the optimizer reads are the only uses of
/// becomes grey, however its resources are written: drawn on two pages, by a form, under two
/// names on one page; through resources that are an object of their own, shared by two pages,
/// whose `/XObject` dictionary is one too; through resources an object of their own with a direct
/// `/XObject` dictionary; through resources a page inherits from the page tree; by a form whose
/// `/Resources` is a reference; by a form without resources, through its page's; listed under a
/// second name nothing draws; through resources an object of their own that a page without
/// content lists too; and with an object nothing in the document reaches referring to it as well.
#[test]
fn black_and_white_rgb_drawn_only_where_the_optimizer_reads_becomes_grey() {
    let (mut doc, image) = rgb_page_fixture();
    let [everywhere, shared_resources, resources_object, inherited, form_resources_object, no_resources, spare_name, no_contents, unreachable] =
        [(); 9].map(|_| image(&mut doc));
    let set_resources = |doc: &mut Document, q: ObjRef, res: Object| doc.update_dict(q, |d| d.set(b"Resources".to_vec(), res)).unwrap();

    let form =
        drawing(&mut doc, Some(Object::Dict(resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", everywhere)])))]))), &form_entries());
    page(&mut doc, &[("E", everywhere), ("F", form)], &draw_each(&["E", "F"]));
    page(&mut doc, &[("E", everywhere), ("G", everywhere)], &draw_each(&["E", "G"]));

    let x = doc.add(Object::Dict(xobject_dict(&[("Im", shared_resources)])));
    let res = doc.add(Object::Dict(resources_of(vec![("XObject", Object::Ref(x))])));
    for _ in 0..2 {
        let q = page(&mut doc, &[], &draw_each(&["Im"]));
        set_resources(&mut doc, q, Object::Ref(res));
    }
    let res = doc.add(Object::Dict(resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", resources_object)])))])));
    let q = page(&mut doc, &[], &draw_each(&["Im"]));
    set_resources(&mut doc, q, Object::Ref(res));

    let q = page(&mut doc, &[], &draw_each(&["I"]));
    doc.update_dict(q, |d| {
        d.remove(b"Resources");
    })
    .unwrap();
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    set_resources(&mut doc, pages, Object::Dict(resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("I", inherited)])))])));

    let res = doc.add(Object::Dict(resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", form_resources_object)])))])));
    let form = drawing(&mut doc, Some(Object::Ref(res)), &form_entries());
    page(&mut doc, &[("F", form)], &draw_each(&["F"]));
    let form = drawing(&mut doc, None, &form_entries());
    page(&mut doc, &[("F", form), ("Im", no_resources)], &draw_each(&["F"]));

    page(&mut doc, &[("C", spare_name), ("D", spare_name)], &draw_each(&["C"]));
    let res = doc.add(Object::Dict(resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", no_contents)])))])));
    for drawn in [true, false] {
        let q = page(&mut doc, &[], &draw_each(&["Im"]));
        set_resources(&mut doc, q, Object::Ref(res));
        if !drawn {
            doc.update_dict(q, |d| {
                d.remove(b"Contents");
            })
            .unwrap();
        }
    }

    page(&mut doc, &[("Im", unreachable)], &draw_each(&["Im"]));
    // A form nothing refers to, drawing it under DefaultGray.
    let orphan = resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", unreachable)]))), ("ColorSpace", default_gray())]);
    drawing(&mut doc, Some(Object::Dict(orphan)), &form_entries());

    let grey = [everywhere, shared_resources, resources_object, inherited, form_resources_object, no_resources, spare_name, no_contents, unreachable];
    assert_eq!(pack_only(&mut doc), (0, grey.len()));
    for i in grey {
        let s = stream(&doc, i);
        assert_eq!((s.dict.get(b"ColorSpace"), s.dict.int(b"BitsPerComponent")), (Some(&Object::name("DeviceGray")), Some(1)), "{i:?}");
    }
}

/// LibreOffice writes one resources object for a page and its `/Pages` node, and a transparency
/// group's form whose `/Resources` is that same object, which the object lists in turn. Black-
/// and-white RGB in it becomes grey: every reference is one the page walk reads, the cycle
/// included. Drawn from outside as well, by an appearance under `/DefaultGray` that draws the
/// form, it stays RGB.
#[test]
fn black_and_white_rgb_in_resources_its_page_tree_and_a_form_share_becomes_grey() {
    for outside in [false, true] {
        let (mut doc, image) = rgb_page_fixture();
        let img = image(&mut doc);
        let res = doc.add(Object::Dict(Dict::new()));
        let form = drawing(&mut doc, Some(Object::Ref(res)), &form_entries());
        doc.update_dict(res, |d| d.set(b"XObject".to_vec(), Object::Dict(xobject_dict(&[("Im", img), ("Tr", form)])))).unwrap();
        let p = page(&mut doc, &[], &draw_each(&["Im", "Tr"]));
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        for holder in [p, pages] {
            doc.update_dict(holder, |d| d.set(b"Resources".to_vec(), Object::Ref(res))).unwrap();
        }
        if outside {
            let gray = resources_of(vec![("XObject", Object::Dict(xobject_dict(&[("Im", form)]))), ("ColorSpace", default_gray())]);
            let appearance = drawing(&mut doc, Some(Object::Dict(gray)), &form_entries());
            annotate(&mut doc, p, appearance);
        }
        let before = stream(&doc, img);
        pack_only(&mut doc);
        let s = stream(&doc, img);
        if outside {
            assert_eq!((&s.dict, &s.raw), (&before.dict, &before.raw), "drawn from outside too");
        } else {
            assert_eq!((s.dict.get(b"ColorSpace"), s.dict.int(b"BitsPerComponent")), (Some(&Object::name("DeviceGray")), Some(1)));
        }
    }
}

/// Dense local mixed-sample patches are retained conservatively by the monochrome settings.
/// One-bit Flate, CCITT and masks preserve samples; eight-bit bilevel inputs may be packed
/// losslessly. Disable the independent gray/color reduction for the exact-render comparison:
/// those settings can otherwise choose smaller grayscale/JPEG output for eight-bit sources.
/// The Retain case separately checks that gray/color reduction remains available.
#[test]
fn ambiguous_monochrome_text_is_recompressed_without_lossy_reduction() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200u32, 480u32);
    let text = bilevel(w, h, false);
    let grey = [("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))];
    let g4 = |doc: &mut Document, entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("CCITTFaxDecode")]));
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(-1));
        parms.set(b"Columns".to_vec(), Object::Int(w.into()));
        d.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Dict(parms)]));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        doc.add(Object::Stream(Stream::from_raw(d, pdfcraft_filters::encode_g4(&text, w, h).unwrap())))
    };
    // Group 4 behind ASCIIHexDecode, its parameters second in the array.
    let behind_hex = |doc: &mut Document| {
        let r = g4(doc, &grey);
        let s = stream(doc, r);
        let mut d = s.dict.clone();
        let parms = d.get(b"DecodeParms").and_then(Object::as_array).and_then(|a| a.first()).cloned().unwrap();
        d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("CCITTFaxDecode")]));
        d.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, parms]));
        let hex = s.raw.iter().map(|b| format!("{b:02x}")).chain([">".to_string()]).collect::<String>().into_bytes();
        doc.set(r, Object::Stream(Stream::from_raw(d, hex)));
        r
    };
    let flate = mono_image(&mut doc, w, h, &text, None, &grey);
    let fax = g4(&mut doc, &grey);
    let hex_fax = behind_hex(&mut doc);
    let hex_at_400 = behind_hex(&mut doc);
    let decode = Object::Array(vec![1.into(), 0.into()]);
    let mask = mono_image(&mut doc, w, h, &text, None, &[("ImageMask", Object::Bool(true)), ("Decode", decode.clone())]);
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w.into()));
    d.set(b"Height".to_vec(), Object::Int(h.into()));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let eight = doc.add(Object::Stream(Stream::flate(d.clone(), &eight_bit(&text, w, h))));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    let rgb = doc.add(Object::Stream(Stream::flate(d.clone(), &samples_of(&text, w, h, [&[0, 0, 0], &[255, 255, 255]], 8))));
    d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0].map(Object::Int).to_vec()));
    let rgb_inverted = doc.add(Object::Stream(Stream::flate(d, &samples_of(&text, w, h, [&[255, 255, 255], &[0, 0, 0]], 8))));
    let at_400 = mono_image(&mut doc, w, h, &text, None, &grey);
    let fax_at_400 = g4(&mut doc, &grey);
    let mut sm = Dict::new();
    sm.set(b"Subtype".to_vec(), Object::name("Image"));
    sm.set(b"Width".to_vec(), Object::Int(2));
    sm.set(b"Height".to_vec(), Object::Int(1));
    sm.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    sm.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let smask = doc.add(Object::Stream(Stream::flate(sm, &[255, 200])));
    let soft = mono_image(&mut doc, w, h, &text, None, &[grey[0].clone(), grey[1].clone(), ("SMask", Object::Ref(smask))]);
    // 144 × 57.6 pt is 2 × 0.8 in: 600 ppi. 216 × 86.4 pt is 400 ppi.
    page(
        &mut doc,
        &[
            ("A", flate),
            ("B", fax),
            ("C", mask),
            ("D", eight),
            ("E", at_400),
            ("F", fax_at_400),
            ("G", soft),
            ("H", hex_fax),
            ("I", hex_at_400),
            ("J", rgb),
            ("K", rgb_inverted),
        ],
        "q 144 0 0 57.6 20 700 cm /A Do Q q 144 0 0 57.6 200 700 cm /B Do Q q 0 0 1 rg 144 0 0 57.6 380 700 cm /C Do Q \
         q 144 0 0 57.6 20 600 cm /D Do Q q 216 0 0 86.4 200 580 cm /E Do Q q 216 0 0 86.4 20 480 cm /F Do Q \
         q 144 0 0 57.6 260 480 cm /G Do Q q 144 0 0 57.6 430 600 cm /H Do Q q 216 0 0 86.4 20 360 cm /I Do Q q 144 0 0 57.6 260 380 cm /J Do Q \
         q 144 0 0 57.6 430 480 cm /K Do Q",
    );
    let before = render(&doc);
    let raw = |doc: &Document, r: ObjRef| stream(doc, r).raw;
    let size = |doc: &Document, r: ObjRef| {
        let d = stream(doc, r).dict;
        (d.int(b"Width").unwrap(), d.int(b"Height").unwrap())
    };
    // This ragged text fixture has dense local mixed-sample patches. The conservative
    // tone guard retains it too: a false positive costs size, not document content.

    // The grey and colour settings left alone too: an image of more bits would otherwise take
    // them when smaller.
    let mut off = doc.clone();
    let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let mono = MonoSettings { downsample: false, ..MonoSettings::REDUCE };
    let r = optimize(&mut off, &Settings { mono, gray: keep, color: keep, ..Settings::default() }).unwrap();
    assert_eq!(r.images_resampled, 0, "{r:?}");
    assert!([flate, fax, mask, eight, hex_fax, rgb, rgb_inverted].iter().all(|i| size(&off, *i) == (1200, 480)));

    let mut kept = doc.clone();
    let r = optimize(&mut kept, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
    // Not the inverted RGB: the colour settings leave an image with a decode array alone.
    assert_eq!(r.images_resampled, 2, "{r:?}");
    assert_eq!(stream(&kept, flate).dict.name(b"Filter"), Some(&b"FlateDecode"[..]), "Retain: Flate stays Flate");
    for i in [fax, hex_fax] {
        assert_eq!(stream(&kept, i).dict.get(b"Filter"), stream(&doc, i).dict.get(b"Filter"), "Retain keeps the exact filter chain");
    }
    assert_eq!(raw(&kept, hex_at_400), raw(&doc, hex_at_400), "and nothing else changes");
    for i in [flate, fax, hex_fax] {
        assert_eq!((size(&kept, i), kept.decoded_image(&stream(&kept, i)).unwrap()), ((1200, 480), text.clone()), "{i:?}");
    }
    let bits = [eight, rgb, rgb_inverted].map(|i| stream(&kept, i).dict.int(b"BitsPerComponent"));
    assert_eq!(bits, [Some(8); 3], "Retain leaves eight bits to the grey and colour settings");

    let untouched = [fax_at_400].map(|r| raw(&doc, r));
    let hex_len = raw(&doc, hex_at_400).len();
    let r = optimize(&mut doc, &Settings { gray: keep, color: keep, ..Settings::default() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 9), "{r:?}");
    assert_eq!(stream(&doc, rgb).dict.get(b"ColorSpace"), Some(&Object::name("DeviceGray")));
    for i in [flate, fax, mask, eight, hex_fax, rgb] {
        let s = stream(&doc, i);
        assert_eq!((size(&doc, i), s.dict.int(b"BitsPerComponent")), ((1200, 480), if i == mask { None } else { Some(1) }));
        assert!(matches!(s.filters().as_slice(), [(Filter::CcittFax | Filter::Flate, _)]), "{i:?}");
        assert_eq!(doc.decoded_image(&s).unwrap(), text.clone(), "{i:?}");
    }
    assert_eq!(stream(&doc, mask).dict.get(b"Decode"), Some(&decode), "the decode array stays");
    // Grey now, with grey's decode pair, which makes 1 black: its ties are black too.
    let s = stream(&doc, rgb_inverted);
    let inverting = Object::Array(vec![1.into(), 0.into()]);
    assert_eq!(
        (size(&doc, rgb_inverted), s.dict.get(b"ColorSpace"), s.dict.get(b"Decode")),
        ((1200, 480), Some(&Object::name("DeviceGray")), Some(&inverting))
    );
    assert_eq!(doc.decoded_image(&s).unwrap(), text.iter().map(|b| !b).collect::<Vec<u8>>(), "the same picture, ties black");
    for i in [at_400, soft, hex_at_400] {
        assert_eq!((size(&doc, i), stream(&doc, i).dict.name(b"Filter")), ((1200, 480), Some(&b"CCITTFaxDecode"[..])), "{i:?}");
    }
    assert!(raw(&doc, hex_at_400).len() < hex_len, "the ASCIIHexDecode is dropped");
    assert_eq!(doc.decoded_image(&stream(&doc, hex_at_400)).unwrap(), text, "and the picture kept");
    assert_eq!([fax_at_400].map(|r| raw(&doc, r)), untouched);
    let after = render(&doc);
    assert_eq!(after, before, "conservative retention preserves the rendered page exactly");
}

/// Monochrome downsampling leaves a halftone or a dither at its resolution, as most new pixels
/// would cover both black and white and taking the sample covering more would turn its tones
/// solid: a 50% checkerboard, a 25% ordered dither and an error-diffused one, each at 600 ppi.
/// Dense ragged text is retained too. Rows wider than a fax row (`MAX_CCITT_COLUMNS`)
/// are never averaged: the averaging keeps a counter for each column.
#[test]
fn monochrome_downsampling_leaves_halftones_and_overwide_rows_at_their_resolution() {
    let (w, h) = (1200u32, 480u32);
    let stride = (w as usize).div_ceil(8);
    let pattern = |white: &dyn Fn(usize, usize) -> bool| {
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                if white(x, y) {
                    data[y * stride + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        data
    };
    let diffused = dithered(w, h);
    let grey = [("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))];
    let mut doc = Document::new_empty();
    let text = mono_image(&mut doc, w, h, &bilevel(w, h, false), None, &grey);
    let halftones =
        [pattern(&|x, y| (x + y) % 2 == 0), pattern(&|x, y| x % 2 == 1 || y % 2 == 1), pattern(&|x, y| diffused[y * w as usize + x] == 255)]
            .map(|data| mono_image(&mut doc, w, h, &data, None, &grey));
    // 144 × 57.6 pt is 600 ppi.
    let names = ["T", "A", "B", "C"];
    let content: String = (0..4).map(|i| format!("q 144 0 0 57.6 {} 600 cm /{} Do Q ", 6 + 150 * i, names[i])).collect();
    page(&mut doc, &names.into_iter().zip([text, halftones[0], halftones[1], halftones[2]]).collect::<Vec<_>>(), &content);
    optimize(&mut doc, &Settings::default()).unwrap();
    let size = |r: ObjRef| {
        let d = stream(&doc, r).dict;
        (d.int(b"Width"), d.int(b"Height"))
    };
    assert_eq!(size(text), (Some(1200), Some(480)), "ambiguous dense text is retained conservatively");
    for (i, r) in halftones.iter().enumerate() {
        assert_eq!(size(*r), (Some(1200), Some(480)), "halftone {i}");
    }

    for (columns, resampled) in [(pdfcraft_filters::MAX_CCITT_COLUMNS, 1), (pdfcraft_filters::MAX_CCITT_COLUMNS + 1, 0)] {
        let mut doc = Document::new_empty();
        let white = vec![0xFF; (columns as usize).div_ceil(8) * 2];
        let img = mono_image(&mut doc, columns, 2, &white, None, &grey);
        // Far above 450 ppi both ways.
        page(&mut doc, &[("A", img)], "q 612 0 0 0.1 0 400 cm /A Do Q");
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(r.images_resampled, resampled, "{columns} columns: {r:?}");
    }
}

/// A fax image behind a decompression bomb is left alone: the filters before its decoder may
/// produce no more than a fax code of its size can take (about a byte a pixel), not the 1 GiB
/// limit. Here a Group 4 page at 600 ppi behind Flate, with four megabytes of zeros after its
/// code; without them it is averaged down.
#[test]
fn a_fax_image_behind_a_decompression_bomb_is_left_alone() {
    let (w, h) = (1200u32, 480u32);
    let g4 = pdfcraft_filters::encode_g4(&bilevel(w, h, false), w, h).unwrap();
    for padding in [0, 4 << 20] {
        let mut doc = Document::new_empty();
        let mut code = g4.clone();
        code.resize(g4.len() + padding, 0);
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode"), Object::name("CCITTFaxDecode")]));
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(-1));
        parms.set(b"Columns".to_vec(), Object::Int(w.into()));
        d.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, Object::Dict(parms)]));
        let img = doc.add(Object::Stream(Stream::from_raw(d, pdfcraft_filters::encode_flate(&code))));
        // 144 × 57.6 pt is 600 ppi.
        page(&mut doc, &[("A", img)], "q 144 0 0 57.6 20 600 cm /A Do Q");
        let before = stream(&doc, img).raw;
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        if padding == 0 {
            assert_eq!(r.images_resampled, 0, "ambiguous local tones are retained: {r:?}");
            assert_eq!(doc.decoded_image(&stream(&doc, img)).unwrap(), bilevel(w, h, false));
        } else {
            assert_eq!((r.images_resampled, r.images_recompressed), (0, 0), "{r:?}");
            assert_eq!(stream(&doc, img).raw, before);
        }
    }
}

/// A CCITT image `w` × `h` coded as Group 4 from `bits`, behind ASCIIHexDecode when `hex`.
fn g4_image(doc: &mut Document, bits: &[u8], (w, h): (u32, u32), hex: bool) -> ObjRef {
    let g4 = pdfcraft_filters::encode_g4(bits, w, h).unwrap();
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w.into()));
    d.set(b"Height".to_vec(), Object::Int(h.into()));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let mut parms = Dict::new();
    parms.set(b"K".to_vec(), Object::Int(-1));
    parms.set(b"Columns".to_vec(), Object::Int(w.into()));
    let data = if hex {
        d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("CCITTFaxDecode")]));
        d.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, Object::Dict(parms)]));
        g4.iter().flat_map(|b| format!("{b:02x}").into_bytes()).chain(*b">").collect()
    } else {
        d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
        d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        g4
    };
    doc.add(Object::Stream(Stream::from_raw(d, data)))
}

/// What Monochrome Images leaves alone and what it may still do. A Group 4 halftone above 450 ppi
/// isn't resampled, so it stays byte for byte: alone, by default; behind ASCIIHexDecode, with
/// Retain (by default it is stored again as Group 4 alone). Under Retain an ambiguous Flate scan
/// keeps its bytes and resolution though Group 4 would be smaller. A busy Group 4 image behind ASCIIHexDecode, whose
/// code is more than three times its raster, decodes within a fax code's bound and is stored
/// again as Group 4 alone. Sixteen-bit grey of only 0x0000 and 0xFFFF isn't eight-bit black and
/// white, and keeps its bytes.
#[test]
fn monochrome_images_keep_what_they_should() {
    let (w, h) = (1200u32, 480u32);
    let stride = (w as usize).div_ceil(8);
    let checker: Vec<u8> = (0..h as usize).flat_map(|y| vec![if y % 2 == 0 { 0xAA } else { 0x55 }; stride]).collect();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    // Twelve dark bands a row, their edges ragged by up to five samples, as a scan's are.
    let mut ragged = vec![0xFFu8; stride * h as usize];
    for y in 0..h as usize {
        for band in 0..12 {
            let (x0, x1) = (band * 100 + (next() % 5) as usize, band * 100 + 50 + (next() % 5) as usize);
            for x in x0..x1.min(w as usize) {
                ragged[y * stride + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    let busy: Vec<u8> = (0..256).flat_map(|y| [if y % 2 == 0 { 0xAA } else { 0xFF }; 32]).collect();
    let build = || {
        let mut doc = Document::new_empty();
        let alone = g4_image(&mut doc, &checker, (w, h), false);
        let wrapped = g4_image(&mut doc, &checker, (w, h), true);
        let grey = [("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))];
        let scan = mono_image(&mut doc, w, h, &ragged, None, &grey);
        let fax = g4_image(&mut doc, &busy, (256, 256), true);
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(16));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        let samples: Vec<u8> = eight_bit(&bilevel(w, h, false), w, h).iter().flat_map(|v| [*v, *v]).collect();
        let deep = doc.add(Object::Stream(Stream::flate(d, &samples)));
        // 144 × 57.6 pt is 600 ppi; 46.08 pt for 256 samples is 400 ppi.
        page(
            &mut doc,
            &[("A", alone), ("B", wrapped), ("C", scan), ("D", fax), ("E", deep)],
            "q 144 0 0 57.6 20 700 cm /A Do Q q 144 0 0 57.6 200 700 cm /B Do Q q 144 0 0 57.6 20 600 cm /C Do Q \
             q 46.08 0 0 46.08 200 600 cm /D Do Q q 144 0 0 57.6 20 500 cm /E Do Q",
        );
        (doc, [alone, wrapped, scan, fax, deep])
    };
    let raw = |doc: &Document, r: ObjRef| stream(doc, r).raw;
    let filters = |doc: &Document, r: ObjRef| stream(doc, r).filters().len();

    let (mut doc, [alone, wrapped, scan, fax, deep]) = build();
    let before = [alone, deep].map(|r| raw(&doc, r));
    optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!([alone, deep].map(|r| raw(&doc, r)), before, "a Group 4 halftone alone, and sixteen-bit grey");
    assert_eq!(filters(&doc, wrapped), 1, "a wrapped Group 4 halftone is stored again alone by default");
    assert!(matches!(stream(&doc, scan).filters().as_slice(), [(Filter::CcittFax | Filter::Flate, _)]), "the scan: Group 4 or Flate");
    assert_eq!(doc.decoded_image(&stream(&doc, scan)).unwrap(), ragged, "the scan preserves every sample");
    assert_eq!((stream(&doc, fax).dict.name(b"Filter"), filters(&doc, fax)), (Some(&b"CCITTFaxDecode"[..]), 1), "the busy fax image");

    let (mut doc, [_, wrapped, scan, _, _]) = build();
    let before = raw(&doc, wrapped);
    let r = optimize(&mut doc, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
    assert_eq!(raw(&doc, wrapped), before, "Retain: a wrapped Group 4 halftone stays as it is");
    let s = stream(&doc, scan);
    assert_eq!((s.dict.int(b"Width"), s.dict.name(b"Filter")), (Some(1200), Some(&b"FlateDecode"[..])), "Retain: {r:?}");
    assert_eq!(doc.decoded_image(&s).unwrap(), ragged, "Retain preserves every sample");
}

/// `rows` coded as CCITT Group 3, one-dimensional (`/K 0`): each row its runs in turn, white first,
/// with ITU-T T.4's terminating codes for the few lengths used here. With `on_bytes`, each row
/// starts on a byte: `Some(false)`, with no end-of-line codes (TIFF's Modified Huffman); `Some(true)`,
/// after an end-of-line code that the fill ends on a byte (T.4's fill, as TIFF writes it).
fn group_3(rows: &[&[u32]], mixed: bool, on_bytes: Option<bool>) -> Vec<u8> {
    let code = |white: bool, run: u32| match (white, run) {
        (true, 0) => "00110101",
        (true, 1) => "000111",
        (false, 1) => "010",
        (true, 8) => "10011",
        (true, 16) => "101010",
        (true, 32) => "00011011",
        (false, 8) => "000101",
        (false, 16) => "0000010111",
        (false, 32) => "000001101010",
        _ => panic!("no code here for a run of {run}"),
    };
    let mut bits = String::new();
    for row in rows {
        let codes = mixed || on_bytes == Some(true);
        if on_bytes.is_some() {
            // The end-of-line code is 12 bits.
            while !(bits.len() + if codes { 12 } else { 0 }).is_multiple_of(8) {
                bits.push('0');
            }
        }
        if codes {
            bits.push_str("000000000001");
        }
        // Mixed coding (/K above 0): a 1 bit after the code, the row one-dimensional.
        if mixed {
            bits.push('1');
        }
        row.iter().enumerate().for_each(|(i, run)| bits.push_str(code(i % 2 == 0, *run)));
    }
    bits.as_bytes().chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, d)| b | (u8::from(*d == b'1') << (7 - i)))).collect()
}

/// A Group 3 fax image is stored again as Group 4 when that is smaller, the same picture (to the
/// pixel on the page, against the picture in Flate); with Retain it stays Group 3. One-dimensional (/K 0) and mixed (/K 1, with
/// end-of-line codes) alike, and rows on bytes, with no codes and with codes the fill ends on a byte
/// (without `/EndOfLine`, which reading by the flag misread).
#[test]
fn group_3_fax_images_become_group_4() {
    let (w, h) = (64u32, 64u32);
    let runs: Vec<&[u32]> = [&[32, 32][..], &[16; 4], &[8; 8], &[0, 16, 16, 32]].into_iter().flat_map(|r| std::iter::repeat_n(r, 16)).collect();
    // White is 1, as CCITTFaxDecode gives it without /BlackIs1.
    let mut bits = Vec::new();
    for row in &runs {
        let mut packed = [0u8; 8];
        let mut x = 0;
        for (i, run) in row.iter().enumerate() {
            for p in x..x + *run as usize {
                if i % 2 == 0 {
                    packed[p / 8] |= 0x80 >> (p % 8);
                }
            }
            x += *run as usize;
        }
        bits.extend(packed);
    }
    for (k, on_bytes) in [(0, None), (1, None), (0, Some(false)), (0, Some(true)), (1, Some(true))] {
        let g3 = group_3(&runs, k > 0, on_bytes);
        let (end_of_line, encoded_byte_align) = (k > 0 && on_bytes.is_none(), on_bytes.is_some());
        let p = pdfcraft_filters::CcittParams { k, columns: w.into(), end_of_line, encoded_byte_align, ..Default::default() };
        assert_eq!(pdfcraft_filters::decode_ccitt(&g3, &p, h).unwrap(), bits, "/K {k}, on bytes {on_bytes:?}: the Group 3 data is the picture");

        let mut doc = Document::new_empty();
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        // The page with the picture in Flate, as it should look.
        let mut control = Document::new_empty();
        let flate = control.add(Object::Stream(Stream::flate(d.clone(), &bits)));
        page(&mut control, &[("A", flate)], "q 64 0 0 64 100 600 cm /A Do Q");
        d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(k));
        parms.set(b"EndOfLine".to_vec(), Object::Bool(end_of_line));
        parms.set(b"EncodedByteAlign".to_vec(), Object::Bool(encoded_byte_align));
        parms.set(b"Columns".to_vec(), Object::Int(w.into()));
        parms.set(b"Rows".to_vec(), Object::Int(h.into()));
        d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        let fax = doc.add(Object::Stream(Stream::from_raw(d, g3.clone())));
        page(&mut doc, &[("A", fax)], "q 64 0 0 64 100 600 cm /A Do Q");

        let mut kept = doc.clone();
        optimize(&mut kept, &Settings { mono: RETAIN, ..Settings::default() }).unwrap();
        assert_eq!(stream(&kept, fax).raw[..], g3[..], "/K {k}, on bytes {on_bytes:?}: Retain keeps Group 3");

        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!((r.images_resampled, r.images_recompressed), (0, 1), "/K {k}, on bytes {on_bytes:?}: {r:?}");
        let s = stream(&doc, fax);
        assert_eq!(doc.fax_params(&s).map(|p| p.k), Some(-1), "/K {k}, on bytes {on_bytes:?}: Group 4");
        assert!(s.raw.len() < g3.len(), "{} bytes, from {}", s.raw.len(), g3.len());
        assert_eq!(doc.decoded_image(&s).unwrap(), bits, "/K {k}, on bytes {on_bytes:?}: the same picture");
        assert!(render(&doc) == render(&control), "/K {k}, on bytes {on_bytes:?}: the page looks as the picture does");
    }
}

/// The images of a document that are images, by object number.
fn image_refs(doc: &Document) -> Vec<ObjRef> {
    doc.object_numbers()
        .into_iter()
        .map(|num| ObjRef { num, generation: doc.generation(num) })
        .filter(|r| matches!(&*doc.get(*r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .collect()
}

/// Black-and-white images as lossless JBIG2: the page looks the same to the pixel (so
/// JBIG2Decode gives black as 0, as the encoder assumes), and scan-like text comes out smaller
/// than as Group 4. JBIG2Decode is PDF 1.4, so a PDF 1.3 document gets `/Version /1.4` in its
/// catalog.
#[test]
fn black_and_white_images_become_jbig2() {
    let mut doc = Document::new_empty();
    let (w, h) = (1200, 480);
    let text = bilevel(w, h, false);
    let gray = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    let decode = Object::Array(vec![1.into(), 0.into()]);
    let mask = mono_image(&mut doc, w, h, &text, None, &[("ImageMask", Object::Bool(true)), ("Decode", decode.clone())]);
    let indexed_cs = Object::Array(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        1.into(),
        Object::String(pdfcraft_cos::PdfString::literal(vec![0xC0, 0x10, 0x10, 0xF8, 0xF0, 0xE0])),
    ]);
    let indexed = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", indexed_cs.clone())]);
    page(
        &mut doc,
        &[("G", gray), ("M", mask), ("I", indexed)],
        "q 300 0 0 120 20 660 cm /G Do Q q 0 0.4 0.8 rg 300 0 0 120 320 660 cm /M Do Q q 300 0 0 120 20 520 cm /I Do Q",
    );
    // The same document, as PDF 1.3.
    let mut bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    bytes[..8].copy_from_slice(b"%PDF-1.3");
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.version(), "1.3");
    let before = render(&doc);

    let mut g4 = doc.clone();
    let as_g4 = optimize(&mut g4, &Settings::default()).unwrap();
    let mut jbig2 = doc.clone();
    let settings = Settings { mono: MonoSettings { compression: MonoCompression::Jbig2, ..MonoSettings::REDUCE }, ..Settings::default() };
    let r = optimize(&mut jbig2, &settings).unwrap();
    assert_eq!(r.images_recompressed, 3);
    assert!(r.image_bytes_after < as_g4.image_bytes_after, "JBIG2 {} bytes, Group 4 {}", r.image_bytes_after, as_g4.image_bytes_after);
    let images: Vec<Stream> = image_refs(&jbig2).into_iter().map(|i| stream(&jbig2, i)).collect();
    assert_eq!(images.len(), 3);
    for s in &images {
        assert_eq!((s.dict.name(b"Filter"), s.dict.contains(b"DecodeParms")), (Some(&b"JBIG2Decode"[..]), false));
    }
    assert!(images.iter().any(|s| s.dict.get(b"Decode") == Some(&decode)), "the decode array stays");
    let indexed_kept =
        |s: &Stream| s.dict.get(b"ColorSpace").and_then(Object::as_array).and_then(|a| a.first()).and_then(Object::as_name) == Some(b"Indexed");
    assert!(images.iter().any(indexed_kept), "the colour space stays");
    let catalog = jbig2.root().and_then(|r| jbig2.get(r).as_dict().cloned()).unwrap();
    assert_eq!(catalog.name(b"Version"), Some(&b"1.4"[..]));
    assert!(g4.root().and_then(|r| g4.get(r).as_dict().cloned()).is_some_and(|c| !c.contains(b"Version")), "Group 4 is PDF 1.0");
    assert!(render(&jbig2) == before, "the page looks the same");
    // The integration also supports changing an already-CCITT image to lossless JBIG2.
    let mut from_g4 = g4.clone();
    let converted = optimize(&mut from_g4, &settings).unwrap();
    assert_eq!(converted.images_recompressed, 3);
    assert!(render(&from_g4) == before, "CCITT decoded and re-encoded as JBIG2 keeps the picture");
}

/// A catalog `/Version` given by reference is read through it: a PDF 1.3 file whose catalog says
/// 1.7 by reference keeps 1.7 when its images become JBIG2, rather than being lowered to 1.4; one
/// whose catalog says 1.3 by reference gets 1.4.
#[test]
fn a_catalog_version_given_by_reference_is_read() {
    for (catalog, after) in [("1.7", "1.7"), ("1.3", "1.4")] {
        let mut doc = Document::new_empty();
        let (w, h) = (1200, 480);
        let text = bilevel(w, h, false);
        let im = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
        page(&mut doc, &[("Im", im)], "q 300 0 0 120 20 660 cm /Im Do Q");
        let version = doc.add(Object::name(catalog));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Version".to_vec(), Object::Ref(version))).unwrap();
        let mut bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
        bytes[..8].copy_from_slice(b"%PDF-1.3");
        let mut doc = Document::open(Arc::new(bytes)).unwrap();
        let settings = Settings { mono: MonoSettings { compression: MonoCompression::Jbig2, ..MonoSettings::REDUCE }, ..Settings::default() };
        let r = optimize(&mut doc, &settings).unwrap();
        assert_eq!(r.images_recompressed, 1);
        let root = doc.root().unwrap();
        let v = doc.get(root).as_dict().and_then(|c| c.get(b"Version").cloned()).unwrap();
        assert_eq!(doc.resolve(&v).as_name(), Some(after.as_bytes()), "catalog {catalog} by reference");
    }
}

/// Text as a renderer draws it at 300 dpi: lines 48 rows apart of a few letter shapes, each
/// `#` a 3 × 3 block, three columns apart with a wider gap between words, every copy of a letter
/// the same to the pixel.
fn rendered_text(w: u32, h: u32) -> Vec<u8> {
    const LETTERS: [&[&str]; 5] = [
        &["##", "##", "##", "##", "##", "##", "##", "##", "##", "##"],
        &["......", ".####.", "##..##", "##..##", "##..##", "##..##", ".####."],
        &["##....", "##....", "##....", "#####.", "##..##", "##..##", "##..##", "#####."],
        &["##..##", "##..##", "##..##", "##..##", ".#####"],
        &["##", "##", "..", "##", "##", "##", "##", "##"],
    ];
    let stride = (w as usize).div_ceil(8);
    let mut data = vec![0xFF; stride * h as usize];
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for baseline in (32..h as usize).step_by(48) {
        let mut x = 8;
        while x + 20 < w as usize {
            let letter = LETTERS[(next() % 5) as usize];
            for (dy, row) in letter.iter().enumerate() {
                for (dx, c) in row.bytes().enumerate() {
                    for (py, px) in (0..9).map(|k| (baseline + 1 + 3 * dy + k / 3 - 3 * letter.len(), x + 3 * dx + k % 3)) {
                        if c == b'#' {
                            data[py * stride + px / 8] &= !(0x80 >> (px % 8));
                        }
                    }
                }
            }
            x += 3 * letter[0].len() + 3 + if next() % 5 == 0 { 12 } else { 0 };
        }
    }
    data
}

/// Text whose letters repeat exactly, as rendered text does, becomes JBIG2 symbols: each letter
/// stored once and its copies placed, several times smaller than JBIG2's generic coding of the
/// same image, and the page looks the same.
#[test]
fn rendered_text_becomes_jbig2_symbols() {
    let mut doc = Document::new_empty();
    let (w, h) = (600, 480);
    let text = rendered_text(w, h);
    let im = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    // One image pixel a device pixel at scale 1, so a wrong pixel shows.
    page(&mut doc, &[("Im", im)], "q 600 0 0 480 6 300 cm /Im Do Q");
    let before = render(&doc);
    let settings = Settings { mono: MonoSettings { compression: MonoCompression::Jbig2, ..MonoSettings::REDUCE }, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert_eq!(r.images_recompressed, 1);
    let s = stream(&doc, im);
    assert_eq!(s.dict.name(b"Filter"), Some(&b"JBIG2Decode"[..]));
    let generic = pdfcraft_filters::encode_jbig2_generic(&text, w, h).unwrap();
    assert!(s.raw.len() * 2 < generic.len(), "symbols {} bytes, generic {}", s.raw.len(), generic.len());
    assert!(render(&doc) == before, "the page looks the same");
}

/// A whole 300 dpi letter page of rendered text, more marks than one JBIG2 text region of the
/// decoder PdfCraft renders with takes, still looks the same as JBIG2 symbols.
#[test]
fn a_dense_page_of_rendered_text_looks_the_same_as_jbig2_symbols() {
    let mut doc = Document::new_empty();
    let (w, h) = (2550, 3300);
    let text = rendered_text(w, h);
    let im = mono_image(&mut doc, w, h, &text, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    page(&mut doc, &[("Im", im)], "q 612 0 0 792 0 0 cm /Im Do Q");
    let before = render(&doc);
    let settings = Settings { mono: MonoSettings { compression: MonoCompression::Jbig2, ..MonoSettings::REDUCE }, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert_eq!(r.images_recompressed, 1);
    let s = stream(&doc, im);
    assert_eq!(s.dict.name(b"Filter"), Some(&b"JBIG2Decode"[..]));
    assert!(s.raw.len() * 2 < pdfcraft_filters::encode_jbig2_generic(&text, w, h).unwrap().len(), "symbol coded");
    // The copies take several text regions (segment type 7, ISO/IEC 14492 7.3), read from the
    // segment headers (7.2): number, flags, referred-to count and numbers, page, data length.
    let mut regions = 0;
    let mut at = 0;
    while let Some(header) = s.raw.get(at..at + 6) {
        let refers = usize::from(header[5] >> 5);
        let length = u32::from_be_bytes(s.raw[at + 7 + refers..at + 11 + refers].try_into().unwrap()) as usize;
        regions += usize::from(header[4] & 0x3F == 7);
        at += 11 + refers + length;
    }
    assert!(regions > 1, "{regions} text regions");
    assert!(render(&doc) == before, "the page looks the same");
}

#[test]
fn settings_choose_what_happens() {
    let mut doc = Document::new_empty();
    let big = image(&mut doc, 1200, 600, 1, false, None);
    page(&mut doc, &[("Im1", big)], "q 144 0 0 72 36 600 cm /Im1 Do Q");
    let mut keep = doc.clone();
    // Off: nothing changes (but the thumbnail goes).
    let off = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let r = optimize(&mut keep, &Settings { color: off, gray: off, ..Settings::default() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 0));
    // Lossless: resampled to 300 ppi, Flate.
    let lossless = ImageSettings { downsample: true, target_ppi: 300.0, above_ppi: 450.0, compression: Compression::Flate };
    let r = optimize(&mut doc, &Settings { gray: lossless, ..Settings::default() }).unwrap();
    assert_eq!(r.images_resampled, 1);
    let s = stream(&doc, big);
    assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height"), s.dict.name(b"Filter")), (Some(600), Some(300), Some(&b"FlateDecode"[..])));
}

#[test]
fn discards_and_clean_up() {
    let mut doc = Document::new_empty();
    let pages = page(&mut doc, &[], "BT /F1 12 Tf 72 720 Td (Hello world, hello world, hello world, hello world) Tj ET");
    let root = doc.root().unwrap();
    let mut vp = Dict::new();
    vp.set(b"PrintScaling".to_vec(), Object::name("None"));
    vp.set(b"Duplex".to_vec(), Object::name("Simplex"));
    vp.set(b"HideToolbar".to_vec(), Object::Bool(true));
    let st = doc.add(Object::Dict(Dict::new()));
    doc.update_dict(root, |c| {
        c.set(b"ViewerPreferences".to_vec(), Object::Dict(vp));
        c.set(b"StructTreeRoot".to_vec(), Object::Ref(st));
        c.set(b"MarkInfo".to_vec(), Object::Dict(Dict::new()));
    })
    .unwrap();
    doc.update_dict(pages, |p| p.set(b"StructParents".to_vec(), Object::Int(0))).unwrap();
    let settings = Settings { discard_tags: true, discard_print_settings: true, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert!(r.tags_removed);
    assert_eq!((r.print_settings, r.thumbnails, r.streams_compressed), (2, 1, 1));
    let c = doc.get(root).as_dict().cloned().unwrap();
    assert!(!c.contains(b"StructTreeRoot") && !c.contains(b"MarkInfo"));
    let vp = c.get(b"ViewerPreferences").and_then(|v| v.as_dict().cloned()).unwrap();
    assert!(vp.contains(b"HideToolbar") && !vp.contains(b"Duplex"), "other preferences stay");
}

#[test]
fn space_audit_shares_the_file_out_by_kind() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q BT ET");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    assert_eq!(audit.len(), SpaceCategory::ALL.len());
    let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().clone();
    let (images, content, overhead) = (get(SpaceCategory::Images), get(SpaceCategory::ContentStreams), get(SpaceCategory::DocumentOverhead));
    assert!(images.percent > 80.0, "{audit:?}");
    assert!(content.bytes > 0 && overhead.bytes > 0, "{audit:?}");
    assert_eq!(audit.iter().map(|u| u.bytes).sum::<u64>(), bytes.len() as u64);
    assert!((audit.iter().map(|u| u.percent).sum::<f64>() - 100.0).abs() < 1e-6);
}

#[test]
fn space_audit_counts_images_a_content_stream_dictionary_points_at_as_images() {
    // Content added with Edit ▸ Add content records its image in the content stream's
    // dictionary (/PCAdded); the image is still an image, not part of the content stream.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    let p = page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q");
    let c = doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
    let mut added = Dict::new();
    added.set(b"Kind".to_vec(), Object::name("Image"));
    added.set(b"Image".to_vec(), Object::Ref(img));
    doc.update_dict(c, |d| d.set(b"PCAdded".to_vec(), Object::Dict(added))).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    let images = audit.iter().find(|u| u.category == SpaceCategory::Images).unwrap();
    assert!(images.percent > 80.0, "{audit:?}");
}

#[test]
fn space_audit_counts_what_fields_and_comments_draw_with_by_kind() {
    // A font or image a form field or comment draws with is still a font or image. The form
    // was claimed first, so its fonts made a fillable form look like mostly "Forms".
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let font = |doc: &mut Document| {
        let file = doc.add(Object::Stream(Stream::from_raw(Dict::new(), vec![7; 40_000])));
        let mut desc = Dict::new();
        desc.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        desc.set(b"FontFile2".to_vec(), Object::Ref(file));
        let desc = doc.add(Object::Dict(desc));
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("TrueType"));
        f.set(b"FontDescriptor".to_vec(), Object::Ref(desc));
        doc.add(Object::Dict(f))
    };
    let (field_font, comment_font) = (font(&mut doc), font(&mut doc));
    let icon = image(&mut doc, 200, 200, 3, false, None);
    let appearance = |doc: &mut Document, kind: &str, name: &str, r: ObjRef| {
        let mut entries = Dict::new();
        entries.set(name.as_bytes().to_vec(), Object::Ref(r));
        let mut res = Dict::new();
        res.set(kind.as_bytes().to_vec(), Object::Dict(entries));
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"Resources".to_vec(), Object::Dict(res));
        Object::Ref(doc.add(Object::Stream(Stream::from_raw(d, b"q Q".to_vec()))))
    };
    let annot = |doc: &mut Document, subtype: &str, ap: Object| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d.set(b"P".to_vec(), Object::Ref(p));
        let mut aps = Dict::new();
        aps.set(b"N".to_vec(), ap);
        d.set(b"AP".to_vec(), Object::Dict(aps));
        Object::Ref(doc.add(Object::Dict(d)))
    };
    // A text field's appearance uses the form's default font; a check box's "on" state (one of
    // a dictionary of states) draws an image; a free-text comment has a font of its own.
    let text_ap = appearance(&mut doc, "Font", "F1", field_font);
    let text = annot(&mut doc, "Widget", text_ap);
    let mut states = Dict::new();
    states.set(b"On".to_vec(), appearance(&mut doc, "XObject", "Im0", icon));
    let check = annot(&mut doc, "Widget", Object::Dict(states));
    let comment_ap = appearance(&mut doc, "Font", "F2", comment_font);
    let comment = annot(&mut doc, "FreeText", comment_ap);
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(vec![text.clone(), check.clone(), comment]))).unwrap();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(field_font));
    let mut dr = Dict::new();
    dr.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut form = Dict::new();
    form.set(b"Fields".to_vec(), Object::Array(vec![text, check]));
    form.set(b"DR".to_vec(), Object::Dict(dr));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().bytes;
    assert!(get(SpaceCategory::Fonts) > 80_000, "both fonts are fonts: {audit:?}");
    assert!(get(SpaceCategory::Images) > 2_000, "the check box's image is an image: {audit:?}");
    assert!(get(SpaceCategory::Forms) < 2_000 && get(SpaceCategory::Comments) < 1_000, "{audit:?}");
    assert!(get(SpaceCategory::Forms) > 0 && get(SpaceCategory::Comments) > 0, "the fields and the comment are still counted: {audit:?}");
}

#[test]
fn space_audit_survives_deeply_nested_forms() {
    // Each form draws the next; sorting their resources by kind recursed once per form, so a
    // long enough chain overflowed the stack.
    let mut doc = Document::new_empty();
    let mut next: Option<ObjRef> = None;
    for _ in 0..20_000 {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        if let Some(n) = next {
            let mut x = Dict::new();
            x.set(b"X".to_vec(), Object::Ref(n));
            let mut res = Dict::new();
            res.set(b"XObject".to_vec(), Object::Dict(x));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
        next = Some(doc.add(Object::Stream(Stream::from_raw(d, b"/X Do".to_vec()))));
    }
    page(&mut doc, &[("X", next.unwrap())], "/X Do");
    let audit = audit_space(&doc, 0);
    let forms = audit.iter().find(|u| u.category == SpaceCategory::XObjectForms).unwrap();
    assert!(forms.bytes > 20_000 * 40, "every form is counted, however deep: {audit:?}");
}

#[test]
fn invalid_links_and_unreferenced_destinations_go() {
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let link = |doc: &mut Document, dest: Object| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Link"));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d.set(b"Dest".to_vec(), dest);
        Object::Ref(doc.add(Object::Dict(d)))
    };
    let s = |t: &str| Object::String(pdfcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
    let annots = vec![
        link(&mut doc, Object::Array(vec![Object::Ref(p), Object::name("Fit")])),
        link(&mut doc, Object::Array(vec![Object::Ref(ObjRef::new(999, 0)), Object::name("Fit")])),
        link(&mut doc, s("gone")),
        link(&mut doc, s("kept")),
    ];
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
    // Named destinations "kept" (used) and "unused"; a bookmark to a missing name.
    let fit = Object::Array(vec![Object::Ref(p), Object::name("Fit")]);
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(vec![s("kept"), fit.clone(), s("unused"), fit]));
    let tree = doc.add(Object::Dict(tree));
    let mut names = Dict::new();
    names.set(b"Dests".to_vec(), Object::Ref(tree));
    let outlines = doc.add(Object::Null);
    let mut item = Dict::new();
    item.set(b"Title".to_vec(), s("Lost"));
    item.set(b"Parent".to_vec(), Object::Ref(outlines));
    item.set(b"Dest".to_vec(), s("nowhere"));
    let item = doc.add(Object::Dict(item));
    let mut o = Dict::new();
    o.set(b"First".to_vec(), Object::Ref(item));
    o.set(b"Last".to_vec(), Object::Ref(item));
    doc.set(outlines, Object::Dict(o));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        c.set(b"Names".to_vec(), Object::Dict(names));
        c.set(b"Outlines".to_vec(), Object::Ref(outlines));
    })
    .unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!((report.invalid_links, report.invalid_bookmarks, report.unreferenced_dests), (2, 1, 1));
    let left = doc.get(p).as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap().len();
    assert_eq!(left, 2);
    assert!(doc.get(item).as_dict().unwrap().get(b"Dest").is_none(), "the bookmark stays, without its broken destination");
    // Off: nothing changes.
    let mut again = doc.clone();
    let off = Settings { remove_invalid_links: false, remove_unreferenced_dests: false, ..Settings::default() };
    assert_eq!(optimize(&mut again, &off).unwrap().invalid_links, 0);
}

#[test]
fn progress_is_reported_per_image_and_a_refusal_cancels() {
    use pdfcraft_optimize::{OptimizeError, Stage, optimize_with_progress};
    let build = || {
        let mut doc = Document::new_empty();
        let a = image(&mut doc, 300, 300, 3, false, None);
        let b = image(&mut doc, 300, 300, 1, false, None);
        let c = image(&mut doc, 300, 300, 3, true, None);
        page(&mut doc, &[("A", a), ("B", b), ("C", c)], "q 72 0 0 72 0 0 cm /A Do Q q 72 0 0 72 100 0 cm /B Do Q q 72 0 0 72 200 0 cm /C Do Q");
        doc
    };
    let mut seen = Vec::new();
    let report = optimize_with_progress(&mut build(), &Settings::default(), &mut |s| {
        seen.push(s);
        true
    })
    .unwrap();
    assert_eq!(report.images, 3);
    assert_eq!(
        seen,
        [
            Stage::Images { done: 0, total: 3 },
            Stage::Images { done: 1, total: 3 },
            Stage::Images { done: 2, total: 3 },
            Stage::Images { done: 3, total: 3 },
            Stage::CleanUp
        ]
    );

    // Stopping after the first image.
    let mut calls = 0;
    let r = optimize_with_progress(&mut build(), &Settings::default(), &mut |_| {
        calls += 1;
        calls < 2
    });
    assert!(matches!(r, Err(OptimizeError::Cancelled)), "{r:?}");
    assert_eq!(calls, 2, "nothing runs after the refusal");
}

/// A page with `resources` and, if given, `contents`.
fn raw_page(doc: &mut Document, resources: Object, contents: Option<ObjRef>) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), resources);
    if let Some(c) = contents {
        p.set(b"Contents".to_vec(), Object::Ref(c));
    }
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

/// Builders for the tests below.
fn xobjects(pairs: &[(&str, ObjRef)]) -> Dict {
    let mut x = Dict::new();
    for (n, r) in pairs {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    x
}

fn resources(x: Dict) -> Dict {
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    res
}

fn content(doc: &mut Document, c: &str) -> ObjRef {
    doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())))
}

fn form(doc: &mut Document, resources: Option<Object>, c: &str) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
    if let Some(r) = resources {
        d.set(b"Resources".to_vec(), r);
    }
    doc.add(Object::Stream(Stream::flate(d, c.as_bytes())))
}

fn xobject_names(doc: &Document, resources: &Object) -> Vec<String> {
    let res = doc.resolve(resources);
    let mut n: Vec<String> =
        res.as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect();
    n.sort();
    n
}

fn page_xobjects(doc: &Document, page: ObjRef) -> Vec<String> {
    let res = doc.get(page).as_dict().unwrap().get(b"Resources").unwrap().clone();
    xobject_names(doc, &res)
}

fn count_images(bytes: &[u8]) -> usize {
    let doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
    doc.object_numbers()
        .into_iter()
        .filter(|n| matches!(&*doc.get(ObjRef::new(*n, doc.generation(*n))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .count()
}

#[test]
fn images_and_forms_nothing_draws_are_dropped() {
    let mut doc = Document::new_empty();
    let img = |doc: &mut Document| image(doc, 64, 64, 3, false, None);

    // Edit a PDF deleting an image: the page gets a new content stream without the `Do`; the
    // old one, which still names the image, is no longer used.
    let t1 = img(&mut doc);
    let old = content(&mut doc, "q 72 0 0 72 0 0 cm /ImT Do Q");
    let t = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImT", t1)]))), Some(old));
    let new = content(&mut doc, "");
    doc.update_dict(t, |p| p.set(b"Contents".to_vec(), Object::Ref(new))).unwrap();
    // Dropped: a JPEG a page no longer draws, beside one it does.
    let a1 = img(&mut doc);
    let a2 = image(&mut doc, 64, 64, 3, true, None);
    let c = content(&mut doc, "q 72 0 0 72 0 0 cm /Im1 Do Q");
    let a = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", a1), ("Im2", a2)]))), Some(c));
    // Dropped from resources the page alone refers to, written as an object of their own.
    let s1 = img(&mut doc);
    let own = doc.add(Object::Dict(resources(xobjects(&[("Unused", s1)]))));
    let c = content(&mut doc, "");
    let s = raw_page(&mut doc, Object::Ref(own), Some(c));
    // An image drawn only inside a form stays; one beside it goes.
    let (d3, d4) = (img(&mut doc), img(&mut doc));
    let f = form(&mut doc, None, "/Im3 Do");
    let c = content(&mut doc, "/F Do");
    let d = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("F", f), ("Im3", d3), ("Im4", d4)]))), Some(c));

    // Kept, though the page's own content doesn't name them:
    // - drawn by a form with resources of its own that lack the name (renderers fall back to
    //   the page's);
    let k1 = img(&mut doc);
    let mut procset = Dict::new();
    procset.set(b"ProcSet".to_vec(), Object::Array(vec![Object::name("PDF")]));
    let fk = form(&mut doc, Some(Object::Dict(procset)), "/ImK Do");
    let c = content(&mut doc, "/Fk Do");
    let k = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Fk", fk), ("ImK", k1)]))), Some(c));
    // - drawn by a Type 3 glyph named like an inert key;
    let g6 = img(&mut doc);
    let glyph = content(&mut doc, "/Im6 Do");
    let mut procs = Dict::new();
    procs.set(b"P".to_vec(), Object::Ref(glyph));
    let mut t3 = Dict::new();
    t3.set(b"Type".to_vec(), Object::name("Font"));
    t3.set(b"Subtype".to_vec(), Object::name("Type3"));
    t3.set(b"CharProcs".to_vec(), Object::Dict(procs));
    let mut fonts = Dict::new();
    fonts.set(b"T3".to_vec(), Object::Dict(t3));
    let mut res = resources(xobjects(&[("Im6", g6)]));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let c = content(&mut doc, "BT /T3 1 Tf (P) Tj ET");
    let g = raw_page(&mut doc, Object::Dict(res), Some(c));
    // - after an inline image whose data runs into the operators (no space before `EI`), or
    //   given as the second of two operands;
    let (l1, l2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 00>EI Q q /ImL Do Q /Other /ImL2 Do");
    let l = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImL", l1), ("ImL2", l2)]))), Some(c));
    // - drawn by a form field that is on no page;
    let z1 = img(&mut doc);
    let c = content(&mut doc, "");
    let z = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Z1", z1)]))), Some(c));
    let wap = form(&mut doc, None, "/Z1 Do");
    let mut appearance = Dict::new();
    appearance.set(b"N".to_vec(), Object::Ref(wap));
    let mut widget = Dict::new();
    widget.set(b"Subtype".to_vec(), Object::name("Widget"));
    widget.set(b"AP".to_vec(), Object::Dict(appearance));
    let widget = doc.add(Object::Dict(widget));
    let mut acroform = Dict::new();
    acroform.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(widget)]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acroform))).unwrap();
    // - in resources shared by pages (or with an annotation's appearance), or inherited from the
    //   page tree: left alone;
    let (b1, b2) = (img(&mut doc), img(&mut doc));
    let shared = doc.add(Object::Dict(resources(xobjects(&[("X1", b1), ("X2", b2)]))));
    let c = content(&mut doc, "0 0 m");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let c = content(&mut doc, "/X1 Do");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let (inh, unused_inh) = (img(&mut doc), img(&mut doc));
    let tree = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    doc.update_dict(tree, |d| d.set(b"Resources".to_vec(), Object::Dict(resources(xobjects(&[("Inh", inh), ("Old", unused_inh)]))))).unwrap();
    // - named by another page for its own image.
    let (r1, r2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "");
    let r = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r1)]))), Some(c));
    let c = content(&mut doc, "/Im9 Do");
    raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r2)]))), Some(c));

    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.unused_xobjects, 4, "{report:?}");
    assert!(page_xobjects(&doc, t).is_empty(), "the deleted image goes");
    assert_eq!(page_xobjects(&doc, a), ["Im1"]);
    assert!(xobject_names(&doc, &Object::Ref(own)).is_empty());
    assert!(page_xobjects(&doc, s).is_empty());
    assert_eq!(page_xobjects(&doc, d), ["F", "Im3"]);
    assert_eq!(page_xobjects(&doc, k), ["Fk", "ImK"]);
    assert_eq!(page_xobjects(&doc, g), ["Im6"]);
    assert_eq!(page_xobjects(&doc, l), ["ImL", "ImL2"]);
    assert_eq!(page_xobjects(&doc, z), ["Z1"]);
    assert_eq!(xobject_names(&doc, &Object::Ref(shared)), ["X1", "X2"]);
    assert_eq!(page_xobjects(&doc, tree), ["Inh", "Old"]);
    assert_eq!(page_xobjects(&doc, r), ["Im9"]);
    let after = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!((count_images(&before), count_images(&after)), (17, 13), "the dropped images are gone from the file");
}

#[test]
fn nothing_is_dropped_when_a_stream_cannot_be_checked() {
    // A page with an image nothing draws, and an appearance stream on it.
    let build = |stream: Stream| {
        let mut doc = Document::new_empty();
        let unused = image(&mut doc, 64, 64, 3, false, None);
        let c = content(&mut doc, "");
        let page = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", unused)]))), Some(c));
        let other = doc.add(Object::Stream(stream));
        let mut appearance = Dict::new();
        appearance.set(b"N".to_vec(), Object::Ref(other));
        let mut annot = Dict::new();
        annot.set(b"AP".to_vec(), Object::Dict(appearance));
        doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Dict(annot)]))).unwrap();
        let report = optimize(&mut doc, &Settings::default()).unwrap();
        (report.unused_xobjects, page_xobjects(&doc, page))
    };
    let (gone, kept) = ((1, Vec::<String>::new()), (0, vec!["Im1".to_string()]));
    // Valid compressed data with extra keys.
    let flate = |extra: &[(&str, Object)]| {
        let mut s = Stream::flate(Dict::new(), b"0 0 m");
        s.dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        for (k, v) in extra {
            s.dict.set(k.as_bytes().to_vec(), v.clone());
        }
        s
    };
    let parms = |k: &str, v: Object| {
        let mut p = Dict::new();
        p.set(k.as_bytes().to_vec(), v);
        Object::Dict(p)
    };
    // Readable: the image goes.
    assert_eq!(build(flate(&[])), gone);
    assert_eq!(build(flate(&[("DecodeParms", parms("Predictor", Object::Int(1)))])), gone);
    // The name written with an escape still counts.
    assert_eq!(build(Stream::flate(Dict::new(), b"/Im#31 Do")), kept);
    // Read differently by viewers, or not at all: nothing goes.
    let dangling = Object::Ref(ObjRef::new(999, 0));
    let cases = [
        Stream::from_raw(flate(&[]).dict, b"not zlib data".to_vec()),
        flate(&[("Filter", dangling.clone())]),
        flate(&[("Filter", Object::Array(vec![Object::name("FlateDecode"), dangling.clone()]))]),
        flate(&[("Filter", Object::Array(vec![Object::name("Crypt"), Object::name("FlateDecode")]))]),
        flate(&[("DP", parms("Predictor", Object::Int(1)))]),
        flate(&[("DecodeParms", Object::Array(vec![Object::Dict(Dict::new()), Object::Dict(Dict::new())]))]),
        flate(&[("DecodeParms", parms("Predictor", dangling))]),
        flate(&[("DecodeParms", parms("Predictor", Object::Real(12.5)))]),
        flate(&[("F", Object::name("external.dat"))]),
        // An image codec on a stream drawn as an appearance: viewers decode it as content.
        Stream::from_raw(
            {
                let mut d = Dict::new();
                d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
                d
            },
            vec![0; 8],
        ),
    ];
    for s in cases {
        let dict = s.dict.clone();
        assert_eq!(build(s), kept, "{dict:?}");
    }
}

/// `data` as ASCII85 text with the `~>` end marker, as ASCII85Decode reads it.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_be_bytes(word);
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (v % 85) as u8 + b'!';
            v /= 85;
        }
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

/// `data` as hex digits with the `>` end marker, as ASCIIHexDecode reads it.
fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = data.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out.push(b'>');
    out
}

fn filtered(names: &[&str], raw: Vec<u8>) -> Stream {
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::Array(names.iter().map(|n| Object::name(n)).collect()));
    Stream::from_raw(d, raw)
}

/// A one-page document whose page content is `content`, with the content's reference.
fn with_content(content: Stream) -> (Document, ObjRef) {
    let mut doc = Document::new_empty();
    let pg = page(&mut doc, &[], "");
    let c = doc.add(Object::Stream(content));
    doc.update_dict(pg, |d| d.set(b"Contents".to_vec(), Object::Ref(c))).unwrap();
    (doc, c)
}

fn vector_ops() -> Vec<u8> {
    "0 0 m 612 792 l S\n".repeat(400).into_bytes()
}

#[test]
fn ascii_wrappers_come_off_filtered_streams_and_the_inner_data_is_unchanged() {
    let ops = vector_ops();
    let inner = Stream::flate(Dict::new(), &ops).raw.to_vec();
    for (outer, encoded) in [("ASCII85Decode", ascii85(&inner)), ("ASCIIHexDecode", ascii_hex(&inner))] {
        let source = filtered(&[outer, "FlateDecode"], encoded);
        let (mut doc, c) = with_content(source.clone());
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(r.streams_compressed, 1, "{outer}");
        let after = stream(&doc, c);
        assert_eq!(after.dict.get(b"Filter"), Some(&Object::Array(vec![Object::name("FlateDecode")])), "{outer}");
        assert_eq!(after.raw.to_vec(), inner, "{outer}: the inner Flate bytes are kept exactly");
        assert_eq!(after.decoded_strict().unwrap(), ops, "{outer}");
        assert!(after.raw.len() < source.raw.len(), "{outer}");
    }
}

#[test]
fn streams_the_wrapper_stripping_does_not_touch_stay_unchanged() {
    let inner = Stream::flate(Dict::new(), &vector_ops()).raw.to_vec();
    let wrapped = ascii85(&inner);
    let mut with_parms = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    with_parms.dict.set(b"DecodeParms".to_vec(), Object::Dict(Dict::new()));
    let mut image = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    image.dict.set(b"Subtype".to_vec(), Object::name("Image"));
    let mut objstm = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    objstm.dict.set(b"Type".to_vec(), Object::name("ObjStm"));
    let mut metadata = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    metadata.dict.set(b"Type".to_vec(), Object::name("Metadata"));
    let cases = [
        ("malformed ASCII85", filtered(&["ASCII85Decode", "FlateDecode"], b"{{ not ASCII85 ~>".to_vec())),
        ("malformed ASCIIHex", filtered(&["ASCIIHexDecode", "FlateDecode"], b"G0>".to_vec())),
        ("DecodeParms", with_parms),
        ("single ASCII filter", filtered(&["ASCII85Decode"], wrapped.clone())),
        ("image", image),
        ("object stream", objstm),
        ("metadata", metadata),
        ("crypt after the wrapper", filtered(&["ASCII85Decode", "Crypt", "FlateDecode"], wrapped.clone())),
        ("tiny", filtered(&["ASCII85Decode", "FlateDecode"], ascii85(b"ab"))),
        ("single Flate", Stream::flate(Dict::new(), &vector_ops())),
    ];
    for (label, s) in cases {
        let (mut doc, c) = with_content(s.clone());
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(r.streams_compressed, 0, "{label}");
        assert_eq!(stream(&doc, c), s, "{label}");
    }
}

#[test]
fn a_second_run_leaves_the_stripped_stream_alone() {
    let inner = Stream::flate(Dict::new(), &vector_ops()).raw.to_vec();
    let (mut doc, c) = with_content(filtered(&["ASCII85Decode", "FlateDecode"], ascii85(&inner)));
    optimize(&mut doc, &Settings::default()).unwrap();
    let once = stream(&doc, c);
    let r = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(r.streams_compressed, 0);
    assert_eq!(stream(&doc, c), once);
}

/// A grey image `w` px wide and one row high, every pixel `v`; JPEG or Flate.
fn strip(doc: &mut Document, w: u32, v: u8, jpeg: bool) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let px = vec![v; w as usize];
    let s = if jpeg {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode(&px, w, 1, image::ExtendedColorType::L8).unwrap();
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, out)
    } else {
        Stream::flate(d, &px)
    };
    doc.add(Object::Stream(s))
}

/// A page drawn the way some PDF printers write a page picture: `rows` images one pixel row
/// high, 1000 units wide, scaled into a 24 pt square (so 600 ppi across at 200 px). Row `r`
/// from the top is grey `r`; `edit` changes the content of each row.
fn printed_page(rows: usize, jpeg: impl Fn(usize) -> bool, edit: impl Fn(usize, String) -> String) -> (Document, Vec<ObjRef>) {
    let mut doc = Document::new_empty();
    let mut names = Vec::new();
    let mut refs = Vec::new();
    let mut content = String::from("q 0.024 0 0 0.024 0 0 cm\n");
    for r in 0..rows {
        let img = strip(&mut doc, 200, r as u8, jpeg(r));
        refs.push(img);
        let y = (rows - 1 - r) as f64;
        content += &edit(r, format!("q 1000 0 0 1 0 {y} cm\n/R{r} Do\nQ\n"));
        names.push(format!("R{r}"));
    }
    content += "Q\n";
    let xo: Vec<(&str, ObjRef)> = names.iter().map(String::as_str).zip(refs.iter().copied()).collect();
    let p = page(&mut doc, &xo, &content);
    // No thumbnail: it would be the old content stream, which still names the strips.
    doc.update_dict(p, |d| {
        d.remove(b"Thumb");
    })
    .unwrap();
    (doc, refs)
}

/// The images the first page's content stream draws, by name, with their streams.
fn drawn_images(doc: &Document) -> Vec<Stream> {
    let pages = pdfcraft_annot::page_refs(doc).unwrap();
    let p = doc.get(pages[0]).as_dict().cloned().unwrap();
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let c = match &*doc.resolve(p.get(b"Contents").unwrap()) {
        Object::Stream(s) => s.decoded().unwrap(),
        _ => panic!("contents"),
    };
    pdfcraft_content::parse(&c)
        .ops
        .iter()
        .filter(|o| o.is("Do"))
        .map(|o| match &*doc.resolve(xo.get(o.name(0).unwrap()).unwrap()) {
            Object::Stream(s) => s.clone(),
            _ => panic!("image"),
        })
        .collect()
}

#[test]
fn a_page_picture_printed_as_one_pixel_strips_is_merged_into_one_image() {
    // 120 strips, every other one JPEG and half as wide (as one PDF printer writes them).
    let (mut doc, _) = printed_page(120, |r| r % 2 == 1, |_, s| s);
    let keep = Settings {
        gray: ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain },
        ..Settings::default()
    };
    let r = optimize(&mut doc, &keep).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (1, 120), "{r:?}");
    let drawn = drawn_images(&doc);
    assert_eq!(drawn.len(), 1, "one image instead of 120");
    let img = &drawn[0];
    assert_eq!((img.dict.int(b"Width"), img.dict.int(b"Height")), (Some(200), Some(120)), "the finest resolution both ways");
    let px = img.decoded().unwrap();
    // Each row is its strip's grey, top row first; JPEG rows within a few levels.
    for (row, chunk) in px.chunks(200).enumerate() {
        assert!(chunk.iter().all(|&v| (v as i32 - row as i32).abs() <= 3), "row {row}: {:?}", &chunk[..4]);
    }
    assert_eq!(r.unused_xobjects, 120, "the strips are dropped");
}

#[test]
fn the_mobile_preset_makes_a_printed_page_small() {
    use pdfcraft_optimize::Preset;
    let (doc, _) = printed_page(200, |_| true, |_, s| s);
    let bytes = |d: &Document| write_full(d, &SaveOptions::default()).unwrap().len();
    let before = bytes(&doc);
    let mut mobile = doc.clone();
    let r = optimize(&mut mobile, &Preset::Mobile.settings()).unwrap();
    assert_eq!((r.merged_images, r.images_resampled), (1, 1), "{r:?}");
    let img = &drawn_images(&mobile)[0];
    // 24 pt = 1/3 in at 96 ppi.
    assert_eq!((img.dict.int(b"Width"), img.dict.int(b"Height")), (Some(32), Some(32)));
    assert_eq!(img.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert!(bytes(&mobile) * 10 < before, "{} → {}", before, bytes(&mobile));
    assert_eq!(Preset::of(&Preset::Mobile.settings()), Some(Preset::Mobile));
    assert_eq!(Preset::from_name("mobile"), Some(Preset::Mobile));
}

#[test]
fn images_that_do_not_tile_a_picture_are_left_alone() {
    let unmerged = |doc: &mut Document, settings: &Settings| {
        let r = optimize(doc, settings).unwrap();
        assert_eq!(r.merged_images, 0, "{r:?}");
    };
    let all = Settings::default();
    // A gap between two rows.
    let (mut gap, _) =
        printed_page(40, |_| false, |r, s| if r < 20 { s.replace(&format!(" 0 {} cm", 39 - r), &format!(" 0 {} cm", 41 - r)) } else { s });
    let r = optimize(&mut gap, &all).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (2, 40), "one picture each side of the gap: {r:?}");
    // Rotated rows.
    let (mut turned, _) = printed_page(40, |_| false, |_, s| s.replace("q 1000 0 0 1", "q 0 1 -1000 0"));
    unmerged(&mut turned, &all);
    // Something else drawn between the rows.
    let (mut between, _) = printed_page(40, |_| false, |r, s| if r == 10 { format!("0 0 10 10 re f\n{s}") } else { s });
    let r = optimize(&mut between, &all).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (2, 40), "two runs, each side of the fill: {r:?}");
    // Too few rows, and the option off.
    let (mut few, _) = printed_page(3, |_| false, |_, s| s);
    unmerged(&mut few, &all);
    let (mut off, _) = printed_page(40, |_| false, |_, s| s);
    unmerged(&mut off, &Settings { merge_fragments: false, ..Settings::default() });
}

/// `levels` nested forms with their own resources, the innermost drawing `img` over the unit square.
fn nested_forms(doc: &mut Document, img: ObjRef, levels: usize) -> ObjRef {
    let mut inner = form(doc, Some(Object::Dict(resources(xobjects(&[("Im", img)])))), "/Im Do");
    for _ in 1..levels {
        inner = form(doc, Some(Object::Dict(resources(xobjects(&[("F", inner)])))), "/F Do");
    }
    inner
}

#[test]
fn shared_forms_are_measured_in_each_pages_physical_units() {
    for reverse in [false, true] {
        let mut doc = Document::new_empty();
        let img = image(&mut doc, 720, 720, 3, false, None);
        let outer = nested_forms(&mut doc, img, 3);
        let normal = page(&mut doc, &[("F", outer)], "q 72 0 0 72 0 0 cm /F Do Q");
        let scaled = page(&mut doc, &[("F", outer)], "q 72 0 0 72 0 0 cm /F Do Q");
        doc.update_dict(scaled, |d| d.set(b"UserUnit".to_vec(), Object::Int(10))).unwrap();
        let pages = if reverse { [scaled, normal] } else { [normal, scaled] };
        assert_eq!(effective_resolutions(&doc, &pages)[&img], 72.0, "reverse={reverse}");
        let report = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(report.images_resampled, 0, "{report:?}");
        assert_eq!(stream(&doc, img).dict.int(b"Width"), Some(720));
    }
}

#[test]
fn a_deeply_nested_form_is_measured_at_its_largest_use() {
    // 1200 px drawn 1 in wide (1200 ppi) and then 8 in wide (150 ppi): 150 ppi, so it isn't resampled.
    let small = "q 72 0 0 72 36 36 cm /F Do Q";
    let large = "q 576 0 0 576 18 100 cm /F Do Q";
    for levels in [1, 6, 12] {
        for (pages, label) in [(vec![format!("{small} {large}")], "one page"), (vec![small.into(), large.into()], "two pages")] {
            let mut doc = Document::new_empty();
            let img = image(&mut doc, 1200, 1200, 3, false, None);
            let outer = nested_forms(&mut doc, img, levels);
            for c in &pages {
                page(&mut doc, &[("F", outer)], c);
            }
            let refs = pdfcraft_annot::page_refs(&doc).unwrap();
            assert_eq!(effective_resolutions(&doc, &refs)[&img].round(), 150.0, "{levels} levels, {label}");
            optimize(&mut doc, &Settings::default()).unwrap();
            assert_eq!(stream(&doc, img).dict.int(b"Width"), Some(1200), "{levels} levels, {label}");
        }
    }
}

#[test]
fn a_form_drawing_itself_is_walked_once_per_use() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let f = form(&mut doc, None, "/Im Do q 2 0 0 2 0 0 cm /F Do Q");
    let mut x = xobjects(&[("Im", img)]);
    x.set(b"F".to_vec(), Object::Ref(f));
    let mut d = match &*doc.get(f) {
        Object::Stream(s) => s.clone(),
        _ => unreachable!(),
    };
    d.dict.set(b"Resources".to_vec(), Object::Dict(resources(x)));
    doc.set(f, Object::Stream(d));
    page(&mut doc, &[("F", f)], "q 72 0 0 72 36 36 cm /F Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    // Only the outer use: the cycle isn't followed, and doesn't count as a missed use.
    assert_eq!(effective_resolutions(&doc, &refs)[&img].round(), 1200.0);
}

#[test]
fn a_shared_form_without_resources_draws_each_callers_image() {
    // G has no /Resources, so its /Im is whichever image the form drawing it names.
    let mut doc = Document::new_empty();
    let a = image(&mut doc, 1200, 1200, 3, false, None);
    let b = image(&mut doc, 1200, 1200, 3, false, None);
    let g = form(&mut doc, None, "/Im Do");
    let mut outers = Vec::new();
    for img in [a, b] {
        let mut inner = form(&mut doc, Some(Object::Dict(resources(xobjects(&[("Im", img), ("G", g)])))), "/G Do");
        for _ in 1..6 {
            inner = form(&mut doc, Some(Object::Dict(resources(xobjects(&[("F", inner)])))), "/F Do");
        }
        outers.push(inner);
    }
    page(&mut doc, &[("A", outers[0]), ("B", outers[1])], "q 72 0 0 72 36 36 cm /A Do Q q 576 0 0 576 18 100 cm /B Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &refs);
    assert_eq!(ppi[&a].round(), 1200.0);
    assert_eq!(ppi.get(&b).map(|p| p.round()), Some(150.0));
}

#[test]
fn images_are_not_resampled_when_some_uses_could_not_be_measured() {
    // Too deep: the 13th form isn't followed, so its use of the image is unknown.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let other = image(&mut doc, 1200, 1200, 3, false, None);
    let deep = nested_forms(&mut doc, img, 13);
    page(
        &mut doc,
        &[("F", deep), ("Im", img), ("O", other)],
        "q 72 0 0 72 36 36 cm /Im Do Q q 72 0 0 72 200 36 cm /O Do Q q 576 0 0 576 18 100 cm /F Do Q",
    );
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &refs);
    assert_eq!((ppi[&img], ppi[&other]), (f64::INFINITY, f64::INFINITY));
    optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(stream(&doc, img).dict.int(b"Width"), Some(1200));

    // Too much work: forms without resources (so nothing can be skipped), each drawing the next
    // three times, twelve deep, would be (3^12 - 1) / 2 walks.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let mut names = vec![("Im", img)];
    let mut next = "Im";
    let labels = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
    for (i, label) in labels.iter().enumerate().rev() {
        let s = 1.0 + i as f64 / 100.0;
        let f = form(&mut doc, None, &format!("q {s} 0 0 {s} 0 0 cm /{next} Do Q /{next} Do q 1 0 0 1 1 0 cm /{next} Do Q"));
        names.push((label, f));
        next = label;
    }
    page(&mut doc, &names, "q 72 0 0 72 36 36 cm /F1 Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    assert_eq!(effective_resolutions(&doc, &refs)[&img], f64::INFINITY);
}

/// Each form entry counts against the page's form allowance, even for a form that decodes to
/// nothing and so costs no decoded bytes.
#[test]
fn images_are_not_resampled_when_a_page_enters_more_forms_than_allowed() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let empty = form(&mut doc, None, "");
    let draws = "/E Do ".repeat(pdfcraft_cos::MAX_PAGE_FORMS + 1);
    page(&mut doc, &[("E", empty), ("Im", img)], &format!("q 72 0 0 72 36 36 cm /Im Do Q {draws}"));
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    assert_eq!(effective_resolutions(&doc, &refs)[&img], f64::INFINITY);
}

#[test]
fn compatibility_level_sets_version_unpacks_for_1_4_and_checks_encryption() {
    let mut base = Document::new_empty();
    page(&mut base, &[], "BT /F1 12 Tf 72 720 Td (Compatibility test) Tj ET");
    let root = base.root().unwrap();
    base.update_dict(root, |c| c.set(b"Version".to_vec(), Object::name("1.6"))).unwrap();

    for (compat, expected) in [
        (Compatibility::Pdf1_4, "1.4"),
        (Compatibility::Pdf1_5, "1.5"),
        (Compatibility::Pdf1_6, "1.6"),
        (Compatibility::Pdf1_7, "1.7"),
        (Compatibility::Pdf2_0, "2.0"),
    ] {
        let mut doc = base.clone();
        let settings = Settings { compatibility: compat, ..Settings::default() };
        optimize(&mut doc, &settings).unwrap();
        assert_eq!(doc.version(), expected);
        assert!(!doc.get(root).as_dict().unwrap().contains(b"Version"), "overriding catalog /Version is cleared");
        let bytes = write_full(&doc, &SaveOptions { object_streams: settings.effective_object_streams(), ..SaveOptions::default() }).unwrap();
        assert!(bytes.starts_with(format!("%PDF-{expected}\n").as_bytes()), "{expected}");
        let has_objstm = bytes.windows(7).any(|w| w == b"/ObjStm");
        assert_eq!(has_objstm, compat != Compatibility::Pdf1_4, "{compat:?}");
        let info = pdfcraft_render::inspect(Arc::new(bytes), None).unwrap();
        assert_eq!(info.pdf_version, expected);
    }

    // Disabling object_streams on PDF 1.7 writes a classic cross-reference table.
    let mut doc = base.clone();
    let settings = Settings { compatibility: Compatibility::Pdf1_7, object_streams: false, ..Settings::default() };
    optimize(&mut doc, &settings).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: settings.effective_object_streams(), ..SaveOptions::default() }).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7\n") && !bytes.windows(7).any(|w| w == b"/ObjStm"));

    // Encryption that requires a newer PDF version than the chosen compatibility is refused.
    let mut aes256 = base.clone();
    aes256
        .set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "u",
            owner_password: "o",
            permissions: -1,
            encrypt_metadata: true,
            seed: [1; 32],
        })
        .unwrap();
    assert!(matches!(
        optimize(&mut aes256, &Settings { compatibility: Compatibility::Pdf1_6, ..Settings::default() }),
        Err(OptimizeError::IncompatibleEncryption { required: "1.7", chosen: "1.6" })
    ));
    optimize(&mut aes256, &Settings { compatibility: Compatibility::Pdf2_0, ..Settings::default() }).unwrap();
    assert_eq!(aes256.version(), "2.0");

    let mut aes128 = base;
    aes128
        .set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes128,
            user_password: "u",
            owner_password: "o",
            permissions: -1,
            encrypt_metadata: true,
            seed: [1; 32],
        })
        .unwrap();
    assert!(matches!(
        optimize(&mut aes128, &Settings { compatibility: Compatibility::Pdf1_5, ..Settings::default() }),
        Err(OptimizeError::IncompatibleEncryption { required: "1.6", chosen: "1.5" })
    ));
    optimize(&mut aes128, &Settings { compatibility: Compatibility::Pdf1_6, ..Settings::default() }).unwrap();
}

#[test]
fn image_resolution_keeps_comments_across_content_pieces() {
    for ending in ["", "\n", "\r", "\r\n"] {
        let mut doc = Document::new_empty();
        let visible = image(&mut doc, 20, 20, 3, false, None);
        let hidden = image(&mut doc, 20, 20, 3, false, None);
        let page = page(&mut doc, &[("Visible", visible), ("Hidden", hidden)], "");
        let first = doc
            .add(Object::Stream(Stream::from_raw(Dict::new(), format!("q 72 0 0 72 0 0 cm /Visible Do Q\n% trailing comment{ending}").into_bytes())));
        let second = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 144 0 0 144 0 0 cm /Hidden Do Q\n".to_vec())));
        doc.update_dict(page, |dict| {
            dict.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(first), Object::Int(17), Object::Ref(second)]));
        })
        .unwrap();
        let measured = effective_resolutions(&doc, &[page]);
        assert_eq!(measured.get(&visible), Some(&20.0));
        if ending.is_empty() {
            assert_eq!(measured.len(), 1);
            assert_eq!(measured.get(&hidden), None, "a comment must hide the next piece's first line");
        } else {
            assert_eq!(measured.len(), 2);
            assert_eq!(measured.get(&hidden), Some(&10.0), "a real line ending exposes the image");
        }
    }
}

#[test]
fn a_hostile_image_size_is_left_alone_not_multiplied_into_overflow() {
    // /Width and /Height are file-controlled: 2^32 + 100 wraps to 100 when read as u32, while the
    // real values overflow `w * h * n`. The image must be skipped, not sized by a wrapped product.
    let mut doc = Document::new_empty();
    let side = i64::from(u32::MAX) + 101;
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(side));
    d.set(b"Height".to_vec(), Object::Int(side));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let hostile = doc.add(Object::Stream(Stream::flate(d, &[7u8; 256])));
    page(&mut doc, &[("Im", hostile)], "q 1 0 0 1 10 10 cm /Im Do Q");
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    assert_eq!(stream(&doc, hostile).dict.int(b"Width"), Some(side), "the image is left as it was");
}

#[test]
fn a_hostile_soft_mask_leaves_its_image_unchanged() {
    let mut doc = Document::new_empty();
    let mask = image(&mut doc, 400, 400, 1, false, None);
    let mut hostile = stream(&doc, mask);
    hostile.dict.set(b"Width".to_vec(), Object::Int(i64::from(u32::MAX) + 401));
    doc.set(mask, Object::Stream(hostile.clone()));
    let photo = image(&mut doc, 400, 400, 3, false, Some(mask));
    let before = stream(&doc, photo);
    page(&mut doc, &[("Photo", photo)], "q 72 0 0 72 10 10 cm /Photo Do Q");
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    assert_eq!(stream(&doc, photo), before);
    assert_eq!(stream(&doc, mask), hostile);
}

#[test]
fn indirectly_referenced_destination_is_kept() {
    // A link's /Dest can be an indirect reference to the name string, not the string itself. Reduce
    // File Size read the name only from a direct string, so it deleted a destination the link still
    // used, silently breaking the link.
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let s = |t: &str| Object::String(pdfcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
    // A link whose /Dest is `ref -> "target"`, and an unused destination for contrast.
    let name_ref = Object::Ref(doc.add(s("target")));
    let mut link = Dict::new();
    link.set(b"Type".to_vec(), Object::name("Annot"));
    link.set(b"Subtype".to_vec(), Object::name("Link"));
    link.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
    link.set(b"Dest".to_vec(), name_ref);
    let link = Object::Ref(doc.add(Object::Dict(link)));
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(vec![link]))).unwrap();
    let fit = Object::Array(vec![Object::Ref(p), Object::name("Fit")]);
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(vec![s("target"), fit.clone(), s("unused"), fit]));
    let tree = doc.add(Object::Dict(tree));
    let mut names = Dict::new();
    names.set(b"Dests".to_vec(), Object::Ref(tree));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names))).unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    // Only "unused" is removed; the indirectly-referenced "target" is kept.
    assert_eq!(report.unreferenced_dests, 1);
    let left = named_dest_keys(&doc);
    assert!(left.contains(&b"target".to_vec()), "indirectly referenced destination was kept: {left:?}");
    assert!(!left.contains(&b"unused".to_vec()));
}

#[test]
fn a_destination_named_only_by_a_remote_or_embedded_go_to_is_removed() {
    // The /D of a GoToR or GoToE action names a destination in another document. A local
    // destination of the same name that nothing else uses is unreferenced.
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let s = |t: &str| Object::String(pdfcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
    let link = |doc: &mut Document, action: Dict| {
        let mut link = Dict::new();
        link.set(b"Type".to_vec(), Object::name("Annot"));
        link.set(b"Subtype".to_vec(), Object::name("Link"));
        link.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        link.set(b"A".to_vec(), Object::Dict(action));
        Object::Ref(doc.add(Object::Dict(link)))
    };
    let mut go_to = Dict::new();
    go_to.set(b"S".to_vec(), Object::name("GoTo"));
    go_to.set(b"D".to_vec(), s("local"));
    let mut remote = Dict::new();
    remote.set(b"S".to_vec(), Object::name("GoToR"));
    remote.set(b"F".to_vec(), s("other.pdf"));
    remote.set(b"D".to_vec(), s("remote"));
    let mut target = Dict::new();
    target.set(b"R".to_vec(), Object::name("C"));
    target.set(b"N".to_vec(), s("attached.pdf"));
    let mut embedded = Dict::new();
    embedded.set(b"S".to_vec(), Object::name("GoToE"));
    embedded.set(b"T".to_vec(), Object::Dict(target));
    embedded.set(b"D".to_vec(), s("embedded"));
    let links = vec![link(&mut doc, go_to), link(&mut doc, remote), link(&mut doc, embedded)];
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(links))).unwrap();
    let fit = Object::Array(vec![Object::Ref(p), Object::name("Fit")]);
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(vec![s("embedded"), fit.clone(), s("local"), fit.clone(), s("remote"), fit]));
    let tree = doc.add(Object::Dict(tree));
    let mut names = Dict::new();
    names.set(b"Dests".to_vec(), Object::Ref(tree));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names))).unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.unreferenced_dests, 2);
    assert_eq!(named_dest_keys(&doc), vec![b"local".to_vec()]);
}

/// The keys in the catalog's /Names /Dests name tree (one level, as the optimizer writes it).
fn named_dest_keys(doc: &Document) -> Vec<Vec<u8>> {
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let names = doc.resolve(cat.get(b"Names").unwrap()).as_dict().cloned().unwrap();
    let tree = doc.resolve(names.get(b"Dests").unwrap()).as_dict().cloned().unwrap();
    let arr = doc.resolve(tree.get(b"Names").unwrap()).as_array().cloned().unwrap();
    arr.chunks(2).filter_map(|c| doc.resolve(&c[0]).as_string().map(|s| s.bytes.clone())).collect()
}

/// Page content long enough for LZW codes to grow past 9 bits, where `EarlyChange` matters.
fn lzw_ops() -> Vec<u8> {
    (0..300).flat_map(|i| format!("{} {} m {} {} l S\n", i % 97, i * 7 % 389, i * 13 % 612, i * 31 % 792).into_bytes()).collect()
}

fn lzw(data: &[u8], params: &Params) -> Vec<u8> {
    pdfcraft_filters::encode(&Filter::Lzw, params, data).unwrap()
}

/// `params` as a `/DecodeParms` dictionary, only the entries that differ from the defaults.
fn decode_parms(p: &Params) -> Dict {
    let mut d = Dict::new();
    for (k, v, default) in [
        (&b"Predictor"[..], p.predictor, 1),
        (b"Colors", p.colors, 1),
        (b"BitsPerComponent", p.bits_per_component, 8),
        (b"Columns", p.columns, 1),
        (b"EarlyChange", p.early_change, 1),
    ] {
        if v != default {
            d.set(k.to_vec(), Object::Int(v));
        }
    }
    d
}

/// A 64 × 48 RGB image, LZW-encoded with `params`.
fn lzw_image(doc: &mut Document, params: &Params) -> (ObjRef, Vec<u8>) {
    let (w, h) = (64u32, 48u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 4) as u8, (i / w * 5) as u8, ((i % w + i / w) * 2) as u8]).collect();
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w.into()));
    d.set(b"Height".to_vec(), Object::Int(h.into()));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    d.set(b"Filter".to_vec(), Object::name("LZWDecode"));
    d.set(b"DecodeParms".to_vec(), Object::Dict(decode_parms(params)));
    (doc.add(Object::Stream(Stream::from_raw(d, lzw(&px, params)))), px)
}

/// Settings that leave images to the clean-up: no resampling, no recompression.
fn clean_up_only() -> Settings {
    let keep = ImageSettings { downsample: false, compression: Compression::Retain, ..ImageSettings::REDUCE };
    Settings { color: keep, gray: keep, ..Settings::default() }
}

#[test]
fn lzw_streams_become_flate_and_decode_to_the_same_bytes() {
    let ops = lzw_ops();
    let early = Params::default();
    let late = Params { early_change: 0, ..Params::default() };
    let png = Params { predictor: 15, colors: 3, columns: 64, ..Params::default() };
    let png_late = Params { early_change: 0, ..png.clone() };
    let mut doc = Document::new_empty();
    let content = |doc: &mut Document, p: &Params| {
        let mut d = Dict::new();
        d.set(b"Filter".to_vec(), Object::name("LZWDecode"));
        if *p != Params::default() {
            d.set(b"DecodeParms".to_vec(), Object::Dict(decode_parms(p)));
        }
        doc.add(Object::Stream(Stream::from_raw(d, lzw(&ops, p))))
    };
    let c_early = content(&mut doc, &early);
    let c_late = content(&mut doc, &late);
    let (img, px) = lzw_image(&mut doc, &png);
    let (img_late, _) = lzw_image(&mut doc, &png_late);
    page(&mut doc, &[("Im", img), ("Im2", img_late)], "q 64 0 0 48 72 600 cm /Im Do Q q 64 0 0 48 172 600 cm /Im2 Do Q");
    let pg = pdfcraft_annot::page_refs(&doc).unwrap()[0];
    doc.update_dict(pg, |d| {
        d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(c_early), Object::Ref(c_late), d.get(b"Contents").unwrap().clone()]))
    })
    .unwrap();
    // Each LZW stream reads as its own data before, so a decoder ignoring EarlyChange fails here.
    assert_eq!((stream(&doc, c_early).decoded_strict().unwrap(), stream(&doc, c_late).decoded_strict().unwrap()), (ops.clone(), ops.clone()));
    assert!(lzw(&ops, &early) != lzw(&ops, &late), "the two code widths differ on this data");

    let r = optimize(&mut doc, &clean_up_only()).unwrap();
    assert_eq!(r.lzw_streams, 4, "{r:?}");
    for (label, at, data) in
        [("content", c_early, &ops), ("content, EarlyChange 0", c_late, &ops), ("image", img, &px), ("image, EarlyChange 0", img_late, &px)]
    {
        let after = stream(&doc, at);
        assert_eq!(after.dict.get(b"Filter"), Some(&Object::name("FlateDecode")), "{label}");
        assert_eq!(&after.decoded_strict().unwrap(), data, "{label}");
        let parms = after.dict.get(b"DecodeParms").and_then(Object::as_dict);
        assert!(parms.is_none_or(|p| !p.contains(b"EarlyChange")), "{label}: EarlyChange is LZW's alone");
    }
    // The image keeps its PNG predictor, now applied under Flate.
    let parms = stream(&doc, img).dict.get(b"DecodeParms").and_then(Object::as_dict).cloned().unwrap();
    assert_eq!((parms.int(b"Predictor"), parms.int(b"Colors"), parms.int(b"Columns")), (Some(15), Some(3), Some(64)));
    assert!(!stream(&doc, c_late).dict.contains(b"DecodeParms"), "nothing left to say for the content stream");
}

#[test]
fn a_page_drawn_from_lzw_streams_renders_the_same_after_conversion() {
    let png = Params { predictor: 12, colors: 3, columns: 64, early_change: 0, ..Params::default() };
    let mut doc = Document::new_empty();
    let (img, _) = lzw_image(&mut doc, &png);
    let pg = page(&mut doc, &[("Im", img)], "");
    let ops = b"0.2 0.5 0.8 rg 36 36 300 200 re f q 256 0 0 192 72 400 cm /Im Do Q\n".repeat(4);
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::name("LZWDecode"));
    d.set(b"DecodeParms".to_vec(), Object::Dict(decode_parms(&Params { early_change: 0, ..Params::default() })));
    let c = doc.add(Object::Stream(Stream::from_raw(d, lzw(&ops, &Params { early_change: 0, ..Params::default() }))));
    doc.update_dict(pg, |p| p.set(b"Contents".to_vec(), Object::Ref(c))).unwrap();
    let render = |doc: &Document| {
        let bytes = write_full(doc, &SaveOptions::default()).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
        let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(out.error.is_none(), "{:?}", out.error);
        out.rgba
    };
    let before = render(&doc);
    // The picture and the rectangle are drawn: the page is not blank.
    assert!(before.chunks(4).any(|p| p[..3] != [255, 255, 255]));
    let r = optimize(&mut doc, &clean_up_only()).unwrap();
    assert_eq!(r.lzw_streams, 2, "{r:?}");
    assert_eq!(stream(&doc, c).dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert!(render(&doc) == before, "the page renders pixel for pixel as before");
}

#[test]
fn lzw_inside_a_filter_chain_is_replaced_and_the_other_stages_kept() {
    let ops = lzw_ops();
    let inner = lzw(&ops, &Params::default());
    // An ASCII85 layer outside the LZW goes with it; a JPEG inside it stays encoded as it was.
    let (mut doc, c) = with_content(filtered(&["ASCII85Decode", "LZWDecode"], ascii85(&inner)));
    let photo = image(&mut doc, 40, 30, 3, true, None);
    let jpeg = stream(&doc, photo).raw.to_vec();
    let mut picture = filtered(&["LZWDecode", "DCTDecode"], lzw(&jpeg, &Params::default()));
    picture.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, Object::Null]));
    let pic = doc.add(Object::Stream(picture));
    // An ASCII85 layer over LZW with a PNG predictor: the predictor's parameters move to the one
    // Flate filter left, without EarlyChange.
    let rows: Vec<u8> = (0..40 * 30).map(|i| (i % 40 * 3 + i / 40) as u8).collect();
    let png = Params { predictor: 12, columns: 40, early_change: 0, ..Params::default() };
    let mut predicted = filtered(&["ASCII85Decode", "LZWDecode"], ascii85(&lzw(&rows, &png)));
    predicted.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, Object::Dict(decode_parms(&png))]));
    let pred = doc.add(Object::Stream(predicted));
    let r = optimize(&mut doc, &clean_up_only()).unwrap();
    assert_eq!(r.lzw_streams, 3, "{r:?}");
    let after = stream(&doc, pred);
    assert_eq!(after.dict.get(b"Filter"), Some(&Object::name("FlateDecode")));
    let parms = after.dict.get(b"DecodeParms").and_then(Object::as_dict).cloned().unwrap();
    assert_eq!((parms.int(b"Predictor"), parms.int(b"Columns"), parms.contains(b"EarlyChange")), (Some(12), Some(40), false));
    assert_eq!(after.decoded_strict().unwrap(), rows);
    let after = stream(&doc, c);
    assert_eq!(after.dict.get(b"Filter"), Some(&Object::name("FlateDecode")));
    assert_eq!(after.decoded_strict().unwrap(), ops);
    let after = stream(&doc, pic);
    assert_eq!(after.dict.get(b"Filter"), Some(&Object::Array(vec![Object::name("FlateDecode"), Object::name("DCTDecode")])));
    assert!(!after.dict.contains(b"DecodeParms"), "all null: dropped");
    let inside = pdfcraft_filters::decode(&[(Filter::Flate, Params::default())], &after.raw, 1 << 20).unwrap();
    assert_eq!(inside, jpeg, "the JPEG bytes are kept exactly");
}

#[test]
fn lzw_streams_that_cannot_be_read_or_are_not_asked_for_stay_unchanged() {
    let ops = lzw_ops();
    let good = lzw(&ops, &Params::default());
    // 9-bit codes 511 after a literal: past the table, corrupt.
    let corrupt = vec![0x80, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    assert!(filtered(&["LZWDecode"], corrupt.clone()).decoded_strict().is_err());
    let mut bad_predictor = filtered(&["LZWDecode"], good.clone());
    let mut parms = Dict::new();
    parms.set(b"Predictor".to_vec(), Object::Int(12));
    parms.set(b"Columns".to_vec(), Object::Int(7));
    bad_predictor.dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    let mut indirect_parms = filtered(&["LZWDecode"], good.clone());
    indirect_parms.dict.set(b"DecodeParms".to_vec(), Object::Ref(ObjRef { num: 999, generation: 0 }));
    // Columns the reader can't take as an integer: read as the default 1 here, so the data is
    // predicted with one column and decodes, but a viewer resolving the value would use another.
    let one_column = Params { predictor: 12, ..Params::default() };
    let odd_columns = |columns: Object| {
        let mut s = filtered(&["LZWDecode"], lzw(&ops, &one_column));
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), Object::Int(12));
        parms.set(b"Columns".to_vec(), columns);
        s.dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        s
    };
    assert_eq!(odd_columns(Object::Real(12.5)).decoded_strict().unwrap(), ops, "read with one column");
    let mut objstm = filtered(&["LZWDecode"], good.clone());
    objstm.dict.set(b"Type".to_vec(), Object::name("ObjStm"));
    let mut external = filtered(&["LZWDecode"], good.clone());
    external.dict.set(b"F".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"data.bin".to_vec())));
    let cases = [
        ("corrupt LZW", filtered(&["LZWDecode"], corrupt), Settings::default()),
        ("corrupt under a PNG predictor", bad_predictor, Settings::default()),
        ("indirect DecodeParms", indirect_parms, Settings::default()),
        ("external file", external, Settings::default()),
        ("object stream, rewritten on save", objstm, Settings::default()),
        ("Columns by reference", odd_columns(Object::Ref(ObjRef { num: 9, generation: 0 })), Settings::default()),
        ("Columns not an integer", odd_columns(Object::Real(12.5)), Settings::default()),
        ("crypt filter", filtered(&["Crypt", "LZWDecode"], good.clone()), Settings::default()),
        ("option off", filtered(&["LZWDecode"], good.clone()), Settings { lzw_to_flate: false, ..Settings::default() }),
    ];
    for (label, s, settings) in cases {
        let (mut doc, c) = with_content(s.clone());
        let r = optimize(&mut doc, &settings).unwrap();
        assert_eq!(r.lzw_streams, 0, "{label}");
        assert_eq!(stream(&doc, c), s, "{label}");
    }
    // The same data, readable and asked for, is converted: the cases above fail for their reason.
    let (mut doc, c) = with_content(filtered(&["LZWDecode"], good));
    assert_eq!(optimize(&mut doc, &Settings::default()).unwrap().lzw_streams, 1);
    assert_eq!(stream(&doc, c).decoded_strict().unwrap(), ops);
}

/// Unknown palette polarity is never guessed. Both palette orders and Decode directions
/// retain the original thin rule and exact samples through a full save and render.
#[test]
fn indexed_thin_rules_keep_their_samples_for_both_palette_orders_and_decodes() {
    let (w, h) = (1200u32, 480u32);
    let stride = (w as usize).div_ceil(8);
    let mut gray_data = vec![0xFF; stride * h as usize];
    for y in 0..h as usize {
        gray_data[y * stride + 101 / 8] &= !(0x80 >> (101 % 8));
    }
    let mut gray = Document::new_empty();
    let gray_image = mono_image(&mut gray, w, h, &gray_data, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
    page(&mut gray, &[("Im", gray_image)], "q 144 0 0 57.6 20 600 cm /Im Do Q");
    let gray_render = render(&gray);
    optimize(&mut gray, &Settings::default()).unwrap();
    let gray_stream = stream(&gray, gray_image);
    let gray_samples = gray.decoded_image(&gray_stream).unwrap();
    assert_eq!((gray_stream.dict.int(b"Width"), gray_stream.dict.int(b"Height")), (Some(600), Some(240)));
    assert!(gray_samples.iter().any(|b| *b != 0xFF), "the DeviceGray thin rule survives");
    for white_first in [false, true] {
        for inverted in [false, true] {
            let mut data = vec![0xFF; stride * h as usize];
            for y in 0..h as usize {
                data[y * stride + 101 / 8] &= !(0x80 >> (101 % 8));
            }
            if white_first != inverted {
                data.iter_mut().for_each(|b| *b = !*b);
            }
            let palette = if white_first { vec![255, 0] } else { vec![0, 255] };
            let cs = Object::Array(vec![
                Object::name("Indexed"),
                Object::name("DeviceGray"),
                1.into(),
                Object::String(pdfcraft_cos::PdfString::literal(palette)),
            ]);
            let mut entries = vec![("BitsPerComponent", 1.into()), ("ColorSpace", cs)];
            if inverted {
                entries.push(("Decode", Object::Array(vec![1.into(), 0.into()])));
            }
            let mut doc = Document::new_empty();
            let image = mono_image(&mut doc, w, h, &data, None, &entries);
            page(&mut doc, &[("Im", image)], "q 144 0 0 57.6 20 600 cm /Im Do Q");
            let before = render(&doc);
            assert_eq!(before, gray_render, "the palette image is visually identical to DeviceGray");
            optimize(&mut doc, &Settings::default()).unwrap();
            let s = stream(&doc, image);
            assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(w.into()), Some(h.into())));
            assert_eq!(doc.decoded_image(&s).unwrap(), data);
            let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
            assert_eq!(render(&saved), before, "palette white-first={white_first}, inverted={inverted}");
        }
    }
}

/// White margins must not hide a small screened photograph from the tone guard.
#[test]
fn mixed_text_and_small_halftone_regions_keep_their_tones_with_wide_white_margins() {
    let (w, h) = (1200u32, 480u32);
    let stride = (w as usize).div_ceil(8);
    for side in [8usize, 40, 300] {
        let mut data = vec![0xFF; stride * h as usize];
        for y in 40..40 + side {
            for x in 100..100 + side {
                if (x + y) % 2 == 0 {
                    data[y * stride + x / 8] &= !(0x80 >> (x % 8));
                }
            }
        }
        // A separate one-pixel text/rule stroke makes this a mixed-content scan.
        for y in 380..420 {
            data[y * stride + 801 / 8] &= !(0x80 >> (801 % 8));
        }
        let mut doc = Document::new_empty();
        let image = mono_image(&mut doc, w, h, &data, None, &[("BitsPerComponent", 1.into()), ("ColorSpace", Object::name("DeviceGray"))]);
        page(&mut doc, &[("Im", image)], "q 144 0 0 57.6 20 600 cm /Im Do Q");
        let before = render(&doc);
        optimize(&mut doc, &Settings::default()).unwrap();
        let s = stream(&doc, image);
        assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(w.into()), Some(h.into())));
        assert_eq!(doc.decoded_image(&s).unwrap(), data, "screened samples survive, not a solid patch");
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(render(&saved), before, "local halftone {side}x{side}");
    }
}
