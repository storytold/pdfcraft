use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use super::*;

fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

fn pages(doc: &Document) -> Vec<Dict> {
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
    kids.iter().map(|k| doc.resolve(k).as_dict().cloned().unwrap()).collect()
}

fn media(d: &Dict) -> Vec<f64> {
    d.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect()
}

/// A tiny PNG: 4×2 RGBA with one transparent pixel, 144 dpi.
fn png_bytes(alpha: bool) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 4, 2);
        enc.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: 5669, yppu: 5669, unit: png::Unit::Meter }));
        let mut w = enc.write_header().unwrap();
        let n = if alpha { 4 } else { 3 };
        let mut data = vec![200u8; 8 * n];
        if alpha {
            data[3] = 0;
        }
        w.write_image_data(&data).unwrap();
    }
    out
}

/// A minimal baseline JPEG header (SOI, JFIF at 300 dpi, SOF0 3×2 RGB, EOI). Only the headers
/// matter: the data is embedded as is.
fn jpeg_bytes() -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0x01, 0x2C, 0x01, 0x2C, 0, 0]);
    v.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8, 0, 2, 0, 3, 3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
fn blank_documents() {
    let doc = reopen(&blank(612.0, 792.0, 3).unwrap());
    assert_eq!(pages(&doc).len(), 3);
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 612.0, 792.0]);
    assert!(blank(1.0, 792.0, 1).is_err() && blank(612.0, 792.0, 0).is_err());
}

#[test]
fn images_become_pages_at_their_resolution() {
    let doc =
        from_images(&[("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes()), ("flat.png".into(), png_bytes(false))]).unwrap();
    let doc = reopen(&doc);
    let p = pages(&doc);
    assert_eq!(p.len(), 3);
    // 4×2 px at 144 dpi → 2×1 pt; 3×2 px at 300 dpi → 0.72×0.48 pt.
    let m0 = media(&p[0]);
    assert!((m0[2] - 2.0).abs() < 0.01 && (m0[3] - 1.0).abs() < 0.01, "{m0:?}");
    let m1 = media(&p[1]);
    assert!((m1[2] - 0.72).abs() < 0.01, "{m1:?}");
    let img = |d: &Dict| {
        let x = doc.resolve(d.get(b"Resources").unwrap());
        let xo = doc.resolve(x.as_dict().unwrap().get(b"XObject").unwrap());
        doc.resolve(xo.as_dict().unwrap().get(b"Im0").unwrap()).as_dict().cloned().unwrap()
    };
    assert!(img(&p[0]).contains(b"SMask"), "transparency is kept");
    assert!(!img(&p[2]).contains(b"SMask"));
    assert_eq!(img(&p[1]).name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert_eq!(img(&p[1]).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    let title = doc.resolve(doc.trailer().get(b"Info").unwrap());
    assert_eq!(title.as_dict().unwrap().get(b"Title").unwrap().as_string().unwrap().to_text(), "photo");
    assert!(matches!(from_images(&[("x.gif".into(), b"GIF89a".to_vec())]), Err(CreateError::Image(..))));
}

#[test]
fn text_is_wrapped_and_paginated() {
    let long: String = (0..200).map(|i| format!("Line {i} of a plain text file\n")).collect();
    let doc = reopen(&from_text("notes", &format!("{long}\u{c}After a form feed"), LETTER, 11.0).unwrap());
    let p = pages(&doc);
    assert!(p.len() >= 5, "{} pages", p.len());
    let content = |d: &Dict| {
        let c = doc.resolve(d.get(b"Contents").unwrap());
        let Object::Stream(s) = &*c else { panic!() };
        String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
    };
    assert!(content(&p[0]).contains("(Line 0 of a plain text file) Tj"));
    assert!(content(p.last().unwrap()).contains("(After a form feed) Tj"), "a form feed starts a page");
}

fn image_of(doc: &Document, page: usize) -> Dict {
    let p = &pages(doc)[page];
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    match &*doc.resolve(xo.get(b"Im0").unwrap()) {
        Object::Stream(s) => s.dict.clone(),
        _ => panic!("not an image"),
    }
}

#[test]
fn bmp_gif_and_multi_page_tiff_images() {
    use image::{ImageEncoder, Rgba, RgbaImage};
    // BMP: 3×2 opaque colour.
    let mut bmp = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp).write_image(&[10, 20, 30].repeat(6), 3, 2, image::ExtendedColorType::Rgb8).unwrap();
    // GIF: 2×2 with a transparent pixel.
    let mut gif = Vec::new();
    {
        let mut img = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
        enc.encode(img.as_raw(), 2, 2, image::ExtendedColorType::Rgba8).unwrap();
    }
    // TIFF: two pages, an 8-bit gray one at 144 dpi and a 1-bit one.
    let mut tif = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut tif).unwrap();
        let mut im = enc.new_image::<tiff::encoder::colortype::Gray8>(4, 2).unwrap();
        im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: 144, d: 1 });
        im.write_data(&[128; 8]).unwrap();
        enc.write_image::<tiff::encoder::colortype::Gray8>(2, 2, &[0, 255, 255, 0]).unwrap();
    }
    let doc = from_images(&[("a.bmp".into(), bmp), ("b.gif".into(), gif), ("scan.tif".into(), tif.into_inner())]).unwrap();
    let doc = reopen(&doc);
    assert_eq!(pages(&doc).len(), 4, "one page per BMP and GIF, two for the TIFF");
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 3.0, 2.0]);
    assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    assert!(image_of(&doc, 1).contains(b"SMask"), "GIF transparency");
    assert_eq!(media(&pages(&doc)[2]), [0.0, 0.0, 2.0, 1.0], "4×2 px at 144 dpi");
    assert_eq!(image_of(&doc, 2).name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
    assert!(matches!(from_images(&[("x.webp".into(), b"RIFF0000WEBP".to_vec())]), Err(CreateError::Image(..))));
}

#[test]
fn images_export_as_jpeg_unchanged_and_others_as_png() {
    let doc = reopen(
        &from_images(&[("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes()), ("flat.png".into(), png_bytes(false))]).unwrap(),
    );
    let out = extract_images(&doc, &[0, 1, 2], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let kinds: Vec<(usize, &str, u32, u32)> = out.images.iter().map(|i| (i.page, i.extension, i.width, i.height)).collect();
    assert_eq!(kinds, [(0, "png", 4, 2), (1, "jpg", 3, 2), (2, "png", 4, 2)]);
    assert_eq!(out.images[1].data, jpeg_bytes(), "JPEG data is written as is");
    // The PNG round-trips the pixels, with the soft mask as alpha.
    let dec = png::Decoder::new(std::io::Cursor::new(out.images[0].data.clone()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(&buf[..8], &[200, 200, 200, 0, 200, 200, 200, 200]);
    // Pages filter; small images can be left out.
    assert_eq!(extract_images(&doc, &[2], 0).images.len(), 1);
    assert!(extract_images(&doc, &[0, 1, 2], 3).images.iter().all(|i| i.width.min(i.height) >= 3));
}

#[test]
fn indexed_and_one_bit_images_decode() {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    // Replace the page's image with a 4×1 indexed image (two colours) and add a 1-bit mask.
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let (_, r) = xo.iter().next().map(|(k, v)| (k.clone(), v.as_ref().unwrap())).unwrap();
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(4));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    d.set(
        b"ColorSpace".to_vec(),
        Object::Array(vec![
            Object::name("Indexed"),
            Object::name("DeviceRGB"),
            Object::Int(1),
            Object::String(PdfString::literal(vec![255, 0, 0, 0, 0, 255])),
        ]),
    );
    doc.set(r, Object::Stream(Stream::from_raw(d, vec![0b0101_0000])));
    let out = extract_images(&doc, &[0], 0);
    let dec = png::Decoder::new(std::io::Cursor::new(out.images[0].data.clone()));
    let mut rd = dec.read_info().unwrap();
    let mut buf = vec![0; rd.output_buffer_size().unwrap()];
    rd.next_frame(&mut buf).unwrap();
    assert_eq!(&buf[..12], &[255, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 255]);
}

#[test]
fn jpeg_2000_images_are_embedded_as_is() {
    // A JP2 file: signature, ftyp, jp2h (ihdr 30 × 20, 3 components; resc 5906 px/m ≈ 150 dpi),
    // and a (stub) codestream.
    let bx = |ty: &[u8], payload: &[u8]| {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(payload);
        v
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&20u32.to_be_bytes());
    ihdr.extend_from_slice(&30u32.to_be_bytes());
    ihdr.extend_from_slice(&[0, 3, 7, 7, 0, 0]);
    let mut resc = Vec::new();
    for _ in 0..2 {
        resc.extend_from_slice(&5906u16.to_be_bytes());
        resc.extend_from_slice(&1u16.to_be_bytes());
    }
    resc.extend_from_slice(&[0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"res ", &bx(b"resc", &resc))].concat();
    let file = [JP2_SIGNATURE.to_vec(), bx(b"ftyp", b"jp2 \0\0\0\0jp2 "), bx(b"jp2h", &jp2h), bx(b"jp2c", &[0xFF, 0x4F, 0xFF, 0x51])].concat();
    let doc = reopen(&from_images(&[("photo.jp2".into(), file.clone())]).unwrap());
    let p = &pages(&doc)[0];
    let m = media(p);
    assert!((m[2] - 30.0 * 72.0 / 150.0).abs() < 0.2 && (m[3] - 20.0 * 72.0 / 150.0).abs() < 0.2, "{m:?}");
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty() && out.skipped.len() == 1, "JPX can't be exported yet: {:?}", out.skipped);
    // A raw codestream: the size comes from SIZ.
    let mut siz = vec![0xFF, 0x4F, 0xFF, 0x51, 0, 41, 0, 0];
    for v in [64u32, 48, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    let doc = reopen(&from_images(&[("raw.j2k".into(), siz)]).unwrap());
    assert_eq!(media(&pages(&doc)[0])[2], 64.0);
}
