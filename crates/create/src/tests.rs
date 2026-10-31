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
fn image_resolution_override_changes_size_without_resampling() {
    let images = [("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes())];
    for (dpi, sizes) in [(72.0, [(4.0, 2.0), (3.0, 2.0)]), (300.0, [(0.96, 0.48), (0.72, 0.48)])] {
        let doc = reopen(&from_images_with_resolution(&images, ImageResolution::Dpi(dpi)).unwrap());
        for (page, size) in pages(&doc).iter().zip(sizes) {
            let m = media(page);
            assert!((m[2] - size.0).abs() < 0.001 && (m[3] - size.1).abs() < 0.001);
            let res = doc.resolve(page.get(b"Resources").unwrap());
            let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
            let img = doc.resolve(xo.as_dict().unwrap().get(b"Im0").unwrap());
            let Object::Stream(stream) = &*img else { panic!("expected an image stream") };
            if stream.dict.name(b"Filter") == Some(&b"DCTDecode"[..]) {
                assert_eq!(*stream.raw, jpeg_bytes());
            } else {
                assert_eq!(stream.dict.int(b"Width"), Some(4));
                assert_eq!(stream.dict.int(b"Height"), Some(2));
                assert!(stream.dict.contains(b"SMask"));
            }
        }
    }
    for dpi in [0.0, -1.0, f64::NAN, f64::INFINITY, 1201.0] {
        assert!(matches!(from_images_with_resolution(&images, ImageResolution::Dpi(dpi)), Err(CreateError::Invalid(_))));
    }
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

#[test]
fn text_keeps_leading_spaces_and_tabs() {
    let doc = reopen(&from_text("t", "    indented\n\ttabbed\nplain", LETTER, 11.0).unwrap());
    let p = pages(&doc);
    let c = doc.resolve(p[0].get(b"Contents").unwrap());
    let Object::Stream(s) = &*c else { panic!() };
    let content = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
    assert!(content.contains("(    indented) Tj"), "{content}");
    assert!(content.contains("(    tabbed) Tj"), "{content}");
    assert!(content.contains("(plain) Tj"), "{content}");
}

/// Text Helvetica can't draw is refused with its line and character, never set as `?`; text
/// PdfCraft generates writes such characters as [U+XXXX] and says so.
#[test]
fn text_helvetica_cannot_draw_is_refused_or_escaped() {
    let err = from_text("notes", "Café —\tok\r\n\u{c}第二页 notes", LETTER, 11.0).unwrap_err();
    let CreateError::Invalid(msg) = &err else { panic!("{err:?}") };
    assert!(msg.contains("(line 2)") && msg.contains("\"第\" (U+7B2C)") && msg.contains("Western European"), "{msg}");
    // Western European text, tabs, CRLF and form feeds included, still converts.
    from_text("notes", "Café —\tok\r\n\u{c}Seite zwei: Grüße", LETTER, 11.0).unwrap();
    let (doc, escaped) = from_text_escaped("Summary of Comments on 报告.pdf", "Comment: 签名 ok\nPage 2", LETTER, 11.0).unwrap();
    assert_eq!(escaped, 2);
    let doc = reopen(&doc);
    let p = pages(&doc);
    let c = doc.resolve(p[0].get(b"Contents").unwrap());
    let Object::Stream(s) = &*c else { panic!() };
    let shown = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
    assert!(shown.contains("(Comment: [U+7B7E][U+540D] ok) Tj") && shown.contains(&format!("({ESCAPED_NOTE}) Tj")), "{shown}");
    assert!(!shown.contains('?'), "{shown}");
    // Nothing to escape: no note.
    let (doc, escaped) = from_text_escaped("r", "Plain report", LETTER, 11.0).unwrap();
    assert_eq!(escaped, 0);
    let p = pages(&doc);
    let Object::Stream(s) = &*doc.resolve(p[0].get(b"Contents").unwrap()) else { panic!() };
    assert!(!String::from_utf8_lossy(&s.decoded().unwrap()).contains("U+XXXX"));
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
    // TIFF: two 8-bit gray pages, one at 144 dpi and one of only black and white.
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

/// #4937: a BMP whose header gives the pixel data's offset as 0 is placed as the same image as
/// the intact file, not shifted by the header bytes.
#[test]
fn a_bmp_with_a_zero_pixel_offset_is_placed_like_the_intact_file() {
    use image::ImageEncoder;
    let mut bmp = Vec::new();
    let pixels: Vec<u8> = (0..18).map(|i| i * 13).collect();
    image::codecs::bmp::BmpEncoder::new(&mut bmp).write_image(&pixels, 3, 2, image::ExtendedColorType::Rgb8).unwrap();
    let mut zero = bmp.clone();
    zero[10..14].copy_from_slice(&0u32.to_le_bytes());
    let samples = |data: Vec<u8>| {
        let doc = reopen(&from_images(&[("a.bmp".into(), data)]).unwrap());
        let res = doc.resolve(pages(&doc)[0].get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        let Object::Stream(s) = &*doc.resolve(xo.get(b"Im0").unwrap()) else { panic!("not an image") };
        s.decoded().unwrap()
    };
    assert_eq!(samples(zero), samples(bmp));
}

/// #665 lifted the tiff crate's 256 MiB decode limit; a few bytes claiming a 60000x60000 page
/// (3.6 GB of gray pixels) must still be refused, not allocated.
#[test]
fn a_tiff_claiming_enormous_dimensions_is_refused() {
    let entries: [(u16, u16, u32); 9] = [
        (256, 4, 60_000), // ImageWidth
        (257, 4, 60_000), // ImageLength
        (258, 3, 8),      // BitsPerSample
        (259, 3, 1),      // Compression: none
        (262, 3, 1),      // BlackIsZero
        (273, 4, 122),    // StripOffsets: just past the IFD
        (277, 3, 1),      // SamplesPerPixel
        (278, 4, 60_000), // RowsPerStrip
        (279, 4, 1),      // StripByteCounts
    ];
    let mut tif = b"II*\0".to_vec();
    tif.extend_from_slice(&8u32.to_le_bytes());
    tif.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        tif.extend_from_slice(&tag.to_le_bytes());
        tif.extend_from_slice(&kind.to_le_bytes());
        tif.extend_from_slice(&1u32.to_le_bytes());
        tif.extend_from_slice(&value.to_le_bytes());
    }
    tif.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(tif.len(), 122);
    tif.push(0);
    assert!(matches!(from_images(&[("huge.tif".into(), tif)]), Err(CreateError::Image(..))));
}

/// A PNG of a few bytes whose header claims `w`×`h` 8-bit RGBA pixels, with no pixel data.
fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in bytes {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let body = [kind, data].concat();
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let ihdr = [&w.to_be_bytes()[..], &h.to_be_bytes(), &[8, 6, 0, 0, 0]].concat();
    chunk(b"IHDR", &ihdr);
    // An empty zlib stream: one stored block of no bytes.
    chunk(b"IDAT", &[0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01]);
    chunk(b"IEND", &[]);
    png
}

#[test]
fn a_tiny_png_claiming_huge_dimensions_is_refused_before_allocating() {
    // 68 bytes claiming 13 GiB (and 1 TiB) of pixels: refused before anything is allocated,
    // where it used to abort the process when the allocation failed.
    for (w, h) in [(60_000, 60_000), (1 << 20, 1 << 18)] {
        let png = png_claiming(w, h);
        assert_eq!(png.len(), 68);
        let r = from_images(&[("huge.png".into(), png)]);
        assert!(matches!(&r, Err(CreateError::Image(_, m)) if m.contains("too large")), "{w}x{h}");
    }
}

/// Likewise a PNG: the png crate's limits don't cover the frame buffer, which its header sizes
/// (60000x60000 RGBA is 14.4 GB). Refused before anything is allocated, not after.
#[test]
fn a_png_claiming_enormous_dimensions_is_refused() {
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 60_000, 60_000);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        // A zlib stream holding one zero byte: far too little data, but a well-formed start.
        w.write_chunk(png::chunk::IDAT, &[0x78, 0x01, 0x01, 0x01, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x01, 0x00, 0x01]).unwrap();
    }
    match from_images(&[("huge.png".into(), png)]) {
        Err(CreateError::Image(_, m)) => assert!(m.contains("too large"), "{m}"),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
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

#[test]
fn text_files_are_decoded_by_byte_order_mark_then_utf8_then_code_page() {
    let utf16 = |text: &str, big: bool| -> Vec<u8> {
        let mut v = if big { vec![0xFE, 0xFF] } else { vec![0xFF, 0xFE] };
        for u in text.encode_utf16() {
            v.extend_from_slice(&if big { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        v
    };
    assert_eq!(decode_text("Kış İstanbul".as_bytes()), "Kış İstanbul");
    assert_eq!(decode_text(b"\xEF\xBB\xBFMerhaba"), "Merhaba", "a UTF-8 byte order mark is dropped");
    assert_eq!(decode_text(&utf16("Kış İstanbul\r\nŞubat", false)), "Kış İstanbul\r\nŞubat");
    assert_eq!(decode_text(&utf16("Grüße 日本", true)), "Grüße 日本");
    // Not UTF-8: Windows-1254 for Turkish, 1252 for other Western text (Icelandic included).
    assert_eq!(decode_text(b"K\xFD\xFE \xDDstanbul \xF0\xFC\xE7"), "Kış İstanbul ğüç");
    assert_eq!(decode_text(b"\xDE\xF3r\xF0ur \xE1 \xEDslandi"), "Þórður á íslandi");
    assert_eq!(decode_text(b"caf\xE9 \x80 5"), "café € 5");
    // Odd and broken input never fails.
    assert_eq!(decode_text(&[0xFF, 0xFE, b'A']), "\u{FFFD}");
    assert_eq!(decode_text(b""), "");
}

#[test]
fn source_kind_tells_pdfs_images_and_text_apart() {
    assert_eq!(source_kind("a.bin", b"%PDF-1.7\n"), Some(SourceKind::Pdf));
    assert_eq!(source_kind("a.txt", b"junk before\n%PDF-1.4"), Some(SourceKind::Pdf), "a header after leading junk");
    assert_eq!(source_kind("a", &png_bytes(false)), Some(SourceKind::Image));
    assert_eq!(source_kind("a.txt", &jpeg_bytes()), Some(SourceKind::Image), "bytes win over the name");
    assert_eq!(source_kind("notes.TXT", b"hello"), Some(SourceKind::Text));
    assert_eq!(source_kind("notes.text", b""), Some(SourceKind::Text));
    // Text that merely starts like a BMP stays text; a truncated BMP header is not an image.
    assert_eq!(source_kind("b.txt", b"BMW drivers"), Some(SourceKind::Text));
    assert_eq!(source_kind("b.bmp", b"BM"), None);
    assert_eq!(source_kind("a.docx", b"PK\x03\x04"), None);
    assert_eq!(source_kind("", b""), None);
    // Only the GIF versions Create decodes count as images, so detection never promises a
    // conversion that then fails.
    assert_eq!(source_kind("a.gif", b"GIF8x not a gif"), None);
    assert_eq!(source_kind("notes.txt", b"GIF8 is a word here"), Some(SourceKind::Text));
}

/// BigTIFF (version 43, used for large scans) is detected and converted like a classic TIFF.
#[test]
fn bigtiff_images_become_pages() {
    let mut tif = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new_big(&mut tif).unwrap();
        let mut im = enc.new_image::<tiff::encoder::colortype::Gray8>(4, 2).unwrap();
        im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: 144, d: 1 });
        im.write_data(&[128; 8]).unwrap();
    }
    let tif = tif.into_inner();
    assert!(tif.starts_with(b"II+\0"), "the encoder wrote a BigTIFF");
    assert_eq!(source_kind("scan.tif", &tif), Some(SourceKind::Image));
    let doc = reopen(&from_images(&[("scan.tif".into(), tif)]).unwrap());
    assert_eq!(pages(&doc).len(), 1);
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 2.0, 1.0], "4×2 px at 144 dpi");
}

/// A synthetic ICC profile: a 128-byte header (size, device class, colour space, `acsp`) and
/// some payload. Only the header is read.
fn icc_profile(space: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 300];
    p[..4].copy_from_slice(&300u32.to_be_bytes());
    p[12..16].copy_from_slice(b"prtr");
    p[16..20].copy_from_slice(space);
    p[36..40].copy_from_slice(b"acsp");
    for (i, b) in p.iter_mut().enumerate().skip(128) {
        *b = i as u8;
    }
    p
}

/// A minimal CMYK JPEG header (SOI, Adobe APP14, `profile` in two APP2 ICC_PROFILE chunks,
/// SOF0 3×2 with four components, EOI).
fn cmyk_jpeg_bytes(profile: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xEE, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 2]);
    let (a, b) = profile.split_at(profile.len() / 2);
    // Written out of order: the sequence numbers decide.
    for (seq, part) in [(2u8, b), (1, a)] {
        v.extend_from_slice(&[0xFF, 0xE2]);
        v.extend_from_slice(&(2 + 14 + part.len() as u16).to_be_bytes());
        v.extend_from_slice(b"ICC_PROFILE\0");
        v.extend_from_slice(&[seq, 2]);
        v.extend_from_slice(part);
    }
    v.extend_from_slice(&[0xFF, 0xC0, 0, 20, 8, 0, 2, 0, 3, 4, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

/// The ICC profile behind an image's colour space: (`/N`, `/Alternate`, profile bytes, object).
fn icc_of(doc: &Document, image: &Dict) -> Option<(i64, Vec<u8>, Vec<u8>, Object)> {
    let cs = image.get(b"ColorSpace")?.as_array()?.clone();
    assert_eq!(cs.first().and_then(Object::as_name), Some(&b"ICCBased"[..]));
    let r = cs.get(1)?.clone();
    let Object::Stream(s) = &*doc.resolve(&r) else { panic!("the ICC profile is not a stream") };
    Some((s.dict.int(b"N")?, s.dict.name(b"Alternate")?.to_vec(), s.decoded().unwrap(), r))
}

/// Move the synthetic fixture's frame before its application segments.
fn jpeg_frame_first(mut bytes: Vec<u8>) -> Vec<u8> {
    let at = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
    let frame: Vec<_> = bytes.drain(at..at + 2 + len).collect();
    bytes.splice(2..2, frame);
    bytes
}

#[test]
fn jpeg_fill_bytes_and_metadata_after_frame_are_preserved() {
    let profile = icc_profile(b"CMYK");
    let mut bytes = jpeg_frame_first(cmyk_jpeg_bytes(&profile));
    // Fill before SOF and APP14, including multiple consecutive fill bytes.
    bytes.splice(24..24, [0xFF, 0xFF]);
    bytes.insert(2, 0xFF);
    let doc = reopen(&from_images(&[("filled.jpg".into(), bytes.clone())]).unwrap());
    let image = image_of(&doc, 0);
    let (n, alt, data, _) = icc_of(&doc, &image).unwrap();
    assert_eq!((n, alt.as_slice(), data), (4, &b"DeviceCMYK"[..], profile));
    assert!(image.contains(b"Decode"), "APP14 after SOF still inverts CMYK");
    assert_eq!(extract_images(&doc, &[0], 0).images[0].data, bytes);

    let embedded = jpeg("density.jpg", &jpeg_frame_first(jpeg_bytes())).unwrap();
    assert_eq!(embedded.px, (3, 2));
    assert_eq!(embedded.dpi, (300.0, 300.0), "JFIF after SOF is read");

    // A mismatched profile found after SOF still falls back leniently, as in #729.
    let bytes = jpeg_frame_first(cmyk_jpeg_bytes(&icc_profile(b"RGB ")));
    let doc = reopen(&from_images(&[("mismatch.jpg".into(), bytes)]).unwrap());
    assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]));
}

#[test]
fn jpeg_segment_scan_stops_at_sos_or_eoi() {
    let profile = icc_profile(b"CMYK");
    for marker in [0xDA, 0xD9] {
        let mut bytes = jpeg_frame_first(cmyk_jpeg_bytes(&profile));
        bytes.truncate(bytes.len() - 2);
        // Neither marker needs a segment length to stop metadata scanning. The trailing
        // invalid APP2 would fail if entropy data or bytes after EOI were walked.
        bytes.extend_from_slice(&[0xFF, 0xFF, marker, 0xFF, 0xE2, 0, 1]);
        let embedded = jpeg("boundary.jpg", &bytes).unwrap();
        assert_eq!(embedded.icc, Some(profile.clone()));
        assert_eq!(embedded.px, (3, 2));
    }
}

#[test]
fn jpeg_segment_scan_rejects_truncated_or_invalid_lengths() {
    for tail in [&[0xFF][..], &[0xFF, 0xFF], &[0xFF, 0xE2], &[0xFF, 0xE2, 0], &[0xFF, 0xE2, 0, 0], &[0xFF, 0xE2, 0, 1], &[0xFF, 0xE2, 0, 3]] {
        let mut bytes = jpeg_bytes();
        bytes.truncate(bytes.len() - 2);
        bytes.extend_from_slice(tail);
        assert!(jpeg("truncated.jpg", &bytes).is_err(), "tail: {tail:?}");
    }
}

#[test]
fn embedded_icc_profiles_tag_the_image_colour_space() {
    // A CMYK JPEG keeps its profile as /ICCBased (N 4), its data byte for byte and its Decode.
    let cmyk = icc_profile(b"CMYK");
    let jpeg = cmyk_jpeg_bytes(&cmyk);
    let rgb = icc_profile(b"RGB ");
    let png = {
        let mut out = Vec::new();
        let mut info = png::Info::with_size(2, 1);
        info.color_type = png::ColorType::Rgb;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = Some(rgb.clone().into());
        let mut w = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
        w.write_image_data(&[10, 20, 30, 40, 50, 60]).unwrap();
        w.finish().unwrap();
        out
    };
    let doc = reopen(&from_images(&[("a.jpg".into(), jpeg.clone()), ("b.jpg".into(), jpeg.clone()), ("c.png".into(), png)]).unwrap());
    let (n, alt, data, first) = icc_of(&doc, &image_of(&doc, 0)).expect("the CMYK JPEG is ICC-tagged");
    assert_eq!((n, alt.as_slice(), data), (4, &b"DeviceCMYK"[..], cmyk.clone()));
    assert!(image_of(&doc, 0).contains(b"Decode"), "Adobe CMYK stays inverted");
    let p = &pages(&doc)[0];
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let Object::Stream(s) = &*doc.resolve(xo.get(b"Im0").unwrap()) else { panic!() };
    assert_eq!(*s.raw, jpeg, "the JPEG data is embedded as is");
    let (_, _, _, second) = icc_of(&doc, &image_of(&doc, 1)).unwrap();
    assert_eq!(first, second, "images with the same profile share one profile object");
    // A PNG iCCP profile tags an RGB image (N 3).
    let (n, alt, data, _) = icc_of(&doc, &image_of(&doc, 2)).expect("the PNG is ICC-tagged");
    assert_eq!((n, alt.as_slice(), data), (3, &b"DeviceRGB"[..], rgb.clone()));
    // The colour space is still exported as CMYK / RGB.
    let out = extract_images(&doc, &[0, 1, 2], 0);
    assert_eq!(out.images.len(), 3, "{:?}", out.skipped);

    // A profile for another colour space, a damaged one or a missing chunk: the device space.
    let mut no_acsp = cmyk.clone();
    no_acsp[36] = b'x';
    let missing_chunk = {
        let mut v = cmyk_jpeg_bytes(&cmyk);
        // Drop the first APP2 segment (sequence 2 of 2).
        let at = v.windows(2).position(|w| w == [0xFF, 0xE2]).unwrap();
        let len = u16::from_be_bytes([v[at + 2], v[at + 3]]) as usize;
        v.drain(at..at + 2 + len);
        v
    };
    for (what, bytes) in [("an RGB profile", cmyk_jpeg_bytes(&rgb)), ("no acsp", cmyk_jpeg_bytes(&no_acsp)), ("a missing chunk", missing_chunk)] {
        let doc = reopen(&from_images(&[("x.jpg".into(), bytes)]).unwrap());
        assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]), "{what}");
    }

    // image_xobject (stamps, signatures, inserted images) tags its image the same way.
    let mut doc = Document::new_empty();
    let (r, _) = image_xobject(&mut doc, "a.jpg", &jpeg).unwrap();
    let Object::Stream(s) = &*doc.get(r) else { panic!() };
    assert_eq!(icc_of(&doc, &s.dict).map(|(n, ..)| n), Some(4));
}

/// Replaces page 0's image with the stream `image` builds (it can add objects, e.g. a mask).
fn with_image(doc: &mut Document, image: impl FnOnce(&mut Document) -> Stream) {
    let page = pdfcraft_model::pages(doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let r = xo.iter().next().map(|(_, v)| v.as_ref().unwrap()).unwrap();
    let s = image(doc);
    doc.set(r, Object::Stream(s));
}

fn image_dict(w: i64, h: i64, space: &str, bpc: i64) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w));
    d.set(b"Height".to_vec(), Object::Int(h));
    d.set(b"ColorSpace".to_vec(), Object::name(space));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
    d
}

/// The exported file's extension and its pixels as RGBA.
fn exported_rgba(doc: &Document) -> (&'static str, Vec<u8>) {
    let out = extract_images(doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let img = &out.images[0];
    if img.extension == "jpg" {
        return ("jpg", image::load_from_memory(&img.data).unwrap().into_rgba8().into_raw());
    }
    let dec = png::Decoder::new(std::io::Cursor::new(img.data.clone()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        other => panic!("unexpected {other:?}"),
    };
    ("png", rgba)
}

fn alphas(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect()
}

/// A 4×2 grey JPEG (mid grey).
fn real_jpeg() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode(&[128u8; 8], 4, 2, image::ExtendedColorType::L8).unwrap();
    out
}

#[test]
fn jpeg_images_with_a_soft_mask_export_as_png_with_alpha() {
    // Issue #645: a cut-out photo (JPEG with an /SMask) lost its transparent background.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mask = Stream::from_raw(image_dict(4, 2, "DeviceGray", 8), vec![0, 255, 0, 255, 255, 0, 255, 0]);
        let m = doc.add(Object::Stream(mask));
        let mut d = image_dict(4, 2, "DeviceGray", 8);
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        d.set(b"SMask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, real_jpeg())
    });
    let (ext, rgba) = exported_rgba(&doc);
    assert_eq!(ext, "png", "a JPEG with transparency is written as a PNG");
    assert_eq!(alphas(&rgba), [0, 255, 0, 255, 255, 0, 255, 0]);
    assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[0].abs_diff(128) <= 4 && p[0] == p[1] && p[1] == p[2]), "{rgba:?}");
    // Without a mask the JPEG is still written as is.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| {
        let mut d = image_dict(4, 2, "DeviceGray", 8);
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, real_jpeg())
    });
    let out = extract_images(&doc, &[0], 0);
    assert_eq!((out.images[0].extension, &out.images[0].data), ("jpg", &real_jpeg()));
}

#[test]
/// Both image export paths apply mask intervals independently of the image's component Decode.
fn soft_mask_decode_ranges_become_exported_alpha() {
    for indirect in [false, true] {
        for (range, expected) in [
            ([0.25, 0.75], [64, 128, 191]),
            ([0.75, 0.25], [191, 127, 64]),
            ([1.0, 0.0], [255, 127, 0]),
            ([-1.0, 2.0], [0, 129, 255]),
            ([0.5, 0.5], [128, 128, 128]),
        ] {
            let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
            with_image(&mut doc, |doc| {
                let endpoints = range.into_iter().map(|v| {
                    let value = Object::Real(v);
                    if indirect { Object::Ref(doc.add(value)) } else { value }
                });
                let decode = Object::Array(endpoints.collect());
                let decode = if indirect { Object::Ref(doc.add(decode)) } else { decode };
                let mut mask = image_dict(3, 1, "DeviceGray", 8);
                mask.set(b"Decode".to_vec(), decode);
                let mask = doc.add(Object::Stream(Stream::from_raw(mask, vec![0, 128, 255])));
                let mut image = image_dict(3, 1, "DeviceRGB", 8);
                image.set(b"SMask".to_vec(), Object::Ref(mask));
                image.set(b"Decode".to_vec(), decode_array(&[1.0, 0.0, 0.0, 1.0, 0.0, 1.0]));
                Stream::from_raw(image, [0, 64, 255].repeat(3))
            });
            let doc = reopen(&doc);
            let out = extract_images(&doc, &[0], 0);
            assert!(out.skipped.is_empty(), "{:?}", out.skipped);
            assert_eq!(out.images.len(), 1);
            let pixels = exported_png_pixels(&out.images[0].data);
            assert_eq!(alphas(&pixels), expected, "Decode {range:?}, indirect={indirect}");
            assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[..3] == [255, 64, 255]), "image component Decode stays independent");
            let (ext, bytes) = image_file(&doc, out.images[0].object).unwrap();
            assert_eq!(ext, "png");
            assert_eq!(exported_png_pixels(&bytes), pixels, "Save Image As and Export all images agree");
        }
    }
}

#[test]
/// Packed and JPEG samples map to opacity before resizing to their image's dimensions.
fn soft_mask_decode_maps_packed_and_jpeg_masks_before_resizing() {
    for jpeg in [false, true] {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let mask = if jpeg {
                let mut mask = image_dict(4, 2, "DeviceGray", 8);
                mask.set(b"Filter".to_vec(), Object::name("DCTDecode"));
                mask.set(b"Decode".to_vec(), decode_array(&[0.25, 0.25]));
                Stream::from_raw(mask, real_jpeg())
            } else {
                let mut mask = image_dict(2, 1, "DeviceGray", 1);
                mask.set(b"Decode".to_vec(), decode_array(&[-1.0, 2.0]));
                Stream::from_raw(mask, vec![0b0100_0000])
            };
            let mask = doc.add(Object::Stream(mask));
            let mut image = image_dict(4, 2, "DeviceRGB", 8);
            image.set(b"SMask".to_vec(), Object::Ref(mask));
            Stream::from_raw(image, vec![10; 24])
        });
        // Clipping before resizing differs from resizing first, then clipping to [0, 255].
        let expected = if jpeg { vec![64; 8] } else { vec![0, 64, 191, 255, 0, 64, 191, 255] };
        assert_eq!(alphas(&exported_rgba(&reopen(&doc)).1), expected, "jpeg={jpeg}");
    }
}

#[test]
fn soft_masks_of_another_size_and_stencil_and_colour_key_masks_become_alpha() {
    // A 2×1 soft mask on a 4×2 image is scaled to the image's size.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let m = doc.add(Object::Stream(Stream::from_raw(image_dict(2, 1, "DeviceGray", 8), vec![0, 255])));
        let mut d = image_dict(4, 2, "DeviceRGB", 8);
        d.set(b"SMask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, vec![10; 24])
    });
    let a = alphas(&exported_rgba(&doc).1);
    assert!(a[0] < 64 && a[3] > 192 && a[4] < 64 && a[7] > 192, "{a:?}");

    // A stencil /Mask: its 1 samples hide the image.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut md = Dict::new();
        md.set(b"Type".to_vec(), Object::name("XObject"));
        md.set(b"Subtype".to_vec(), Object::name("Image"));
        md.set(b"Width".to_vec(), Object::Int(4));
        md.set(b"Height".to_vec(), Object::Int(2));
        md.set(b"ImageMask".to_vec(), Object::Bool(true));
        let m = doc.add(Object::Stream(Stream::from_raw(md, vec![0b1100_0000, 0b0011_0000])));
        let mut d = image_dict(4, 2, "DeviceRGB", 8);
        d.set(b"Mask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, vec![10; 24])
    });
    assert_eq!(alphas(&exported_rgba(&doc).1), [0, 0, 255, 255, 255, 255, 0, 0]);

    // A colour-key /Mask: pure white pixels are transparent.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| {
        let mut d = image_dict(2, 1, "DeviceRGB", 8);
        d.set(b"Mask".to_vec(), Object::Array([250, 255, 250, 255, 250, 255].map(Object::Int).to_vec()));
        Stream::from_raw(d, vec![255, 255, 255, 255, 0, 0])
    });
    assert_eq!(exported_rgba(&doc).1, [255, 255, 255, 0, 255, 0, 0, 255]);

    // A soft mask with /Matte: the colours are un-blended from the matte colour.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut md = image_dict(2, 1, "DeviceGray", 8);
        md.set(b"Matte".to_vec(), Object::Array(vec![Object::Int(1)]));
        let m = doc.add(Object::Stream(Stream::from_raw(md, vec![255, 128])));
        let mut d = image_dict(2, 1, "DeviceGray", 8);
        d.set(b"SMask".to_vec(), Object::Ref(m));
        // Black at half alpha, pre-blended with white: 255 + (0 − 255) × 128 / 255 ≈ 127.
        Stream::from_raw(d, vec![0, 127])
    });
    let rgba = exported_rgba(&doc).1;
    assert_eq!(rgba[..4], [0, 0, 0, 255]);
    assert!(rgba[4] <= 2 && rgba[7] == 128, "{rgba:?}");
}

#[test]
fn damaged_masks_and_sizes_never_panic() {
    for (w, h, mw, mh, data) in [(4, 2, -3, 1, vec![]), (4, 2, i64::MAX, 2, vec![0; 8]), (-4, 2, 4, 2, vec![0; 8]), (4, 2, 4, 2, vec![])] {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let m = doc.add(Object::Stream(Stream::from_raw(image_dict(mw, mh, "DeviceGray", 8), data.clone())));
            let mut d = image_dict(w, h, "DeviceRGB", 8);
            d.set(b"SMask".to_vec(), Object::Ref(m));
            d.set(b"Mask".to_vec(), Object::Array(vec![Object::Real(f64::NAN), Object::Int(-5)]));
            Stream::from_raw(d, vec![10; 24])
        });
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        let r = xo.iter().next().map(|(_, v)| v.as_ref().unwrap()).unwrap();
        let _ = extract_images(&doc, &[0], 0);
        let _ = image_file(&doc, r);
    }
}

#[test]
fn indexed_image_export_clamps_samples_to_hival() {
    for palette in [vec![255, 0, 0, 0, 0, 255], vec![255, 0, 0, 0, 0, 255, 0, 255, 0, 255, 255, 255]] {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        let image = xo.iter().next().unwrap().1.as_ref().unwrap();
        let mut dict = Dict::new();
        dict.set(b"Type".to_vec(), Object::name("XObject"));
        dict.set(b"Subtype".to_vec(), Object::name("Image"));
        dict.set(b"Width".to_vec(), Object::Int(4));
        dict.set(b"Height".to_vec(), Object::Int(1));
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(2));
        dict.set(
            b"ColorSpace".to_vec(),
            Object::Array(vec![Object::name("Indexed"), Object::name("DeviceRGB"), Object::Int(1), Object::String(PdfString::literal(palette))]),
        );
        // Default Decode preserves sample values0,1,2,3. Values2 and3 clamp to hival1.
        doc.set(image, Object::Stream(Stream::from_raw(dict, vec![0b0001_1011])));
        let out = extract_images(&doc, &[0], 0);
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(out.images.len(), 1);
        assert_eq!(exported_png_pixels(&out.images[0].data), vec![255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 255]);
    }
}

fn exported_png_pixels(bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    pixels
}

fn indexed_export_fixture(
    hival: Object,
    palette: Vec<u8>,
    stream_table: bool,
    indirect_hival: bool,
    bpc: i64,
    samples: Vec<u8>,
) -> (Document, pdfcraft_cos::ObjRef) {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let image = xo.iter().next().unwrap().1.as_ref().unwrap();
    let table = if stream_table {
        Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &palette))))
    } else {
        Object::String(PdfString::literal(palette))
    };
    let hival = if indirect_hival { Object::Ref(doc.add(hival)) } else { hival };
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Image"));
    dict.set(b"Width".to_vec(), Object::Int(4));
    dict.set(b"Height".to_vec(), Object::Int(1));
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
    dict.set(b"ColorSpace".to_vec(), Object::Array(vec![Object::name("Indexed"), Object::name("DeviceRGB"), hival, table]));
    doc.set(image, Object::Stream(Stream::from_raw(dict, samples)));
    (doc, image)
}

#[test]
fn indexed_hival_resolves_indirect_values_and_stream_palettes() {
    for (bpc, samples) in [(2, vec![0x1b]), (4, vec![0x01, 0x23]), (8, vec![0, 1, 2, 255])] {
        let (doc, image) = indexed_export_fixture(Object::Int(1), vec![255, 0, 0, 0, 0, 255], true, true, bpc, samples);
        let (ext, bytes) = crate::extract::image_file(&doc, image).unwrap();
        assert_eq!(ext, "png");
        assert_eq!(exported_png_pixels(&bytes), vec![255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 255]);
    }
}

#[test]
fn indexed_lookup_stream_has_a_size_cap() {
    // A lookup table needs 256 × 4 bytes at most; a 1 MiB one is refused, not decoded whole.
    let mut palette = vec![255, 0, 0, 0, 0, 255];
    palette.resize(1 << 20, 0);
    let (doc, image) = indexed_export_fixture(Object::Int(1), palette, true, false, 8, vec![0, 1, 0, 1]);
    assert!(crate::extract::image_file(&doc, image).unwrap_err().contains("lookup table"));
}

#[test]
fn indexed_hival_boundary_values_keep_valid_palette_entries() {
    let palette: Vec<u8> = (0..=255u8).flat_map(|i| [i, 255 - i, 17]).collect();
    for high in [0, 255] {
        let (doc, image) = indexed_export_fixture(Object::Int(high), palette.clone(), false, false, 8, vec![0, 1, 254, 255]);
        let (_, bytes) = crate::extract::image_file(&doc, image).unwrap();
        let expected: Vec<u8> = [0u8, 1, 254, 255]
            .into_iter()
            .flat_map(|i| {
                let i = i.min(high as u8);
                [i, 255 - i, 17]
            })
            .collect();
        assert_eq!(exported_png_pixels(&bytes), expected);
    }
}

#[test]
fn indexed_hival_rejects_invalid_values_without_panicking() {
    for high in [Object::Int(-1), Object::Int(256), Object::Real(1.0), Object::Null, Object::name("bad")] {
        let (doc, image) = indexed_export_fixture(high, vec![255, 0, 0], false, false, 8, vec![0; 4]);
        assert!(crate::extract::image_file(&doc, image).unwrap_err().contains("invalid hival"));
        let out = extract_images(&doc, &[0], 0);
        assert!(out.images.is_empty());
        assert_eq!(out.skipped.len(), 1);
        assert!(out.skipped[0].2.contains("invalid hival"));
    }
}

#[test]
fn indexed_hival_clamping_preserves_raw_colour_key_mask_samples() {
    let (mut doc, image) = indexed_export_fixture(Object::Int(1), vec![255, 0, 0, 0, 0, 255], false, false, 2, vec![0x1b]);
    let Object::Stream(mut stream) = (*doc.get(image)).clone() else { panic!("image stream") };
    stream.dict.set(b"Mask".to_vec(), Object::Array(vec![Object::Int(2), Object::Int(2)]));
    doc.set(image, Object::Stream(stream));
    let (_, bytes) = crate::extract::image_file(&doc, image).unwrap();
    assert_eq!(exported_png_pixels(&bytes), vec![255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255, 255]);
}

#[test]
fn indexed_hival_missing_element_returns_export_error() {
    let (mut doc, image) = indexed_export_fixture(Object::Int(1), vec![255, 0, 0], false, false, 8, vec![0; 4]);
    let Object::Stream(mut stream) = (*doc.get(image)).clone() else { panic!("image stream") };
    stream.dict.set(b"ColorSpace".to_vec(), Object::Array(vec![Object::name("Indexed"), Object::name("DeviceRGB")]));
    doc.set(image, Object::Stream(stream));
    assert!(crate::extract::image_file(&doc, image).unwrap_err().contains("invalid hival"));
}

#[test]
fn indexed_hival_keeps_existing_short_palette_fallback() {
    let (doc, image) = indexed_export_fixture(Object::Int(1), vec![255, 0, 0], false, false, 2, vec![0x1b]);
    let (_, bytes) = crate::extract::image_file(&doc, image).unwrap();
    assert_eq!(exported_png_pixels(&bytes), vec![255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
}

/// Save and reopen a synthetic image before checking both export entry points.
fn decode_fixture(dict: Dict, samples: Vec<u8>) -> (Document, pdfcraft_cos::ObjRef) {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| Stream::from_raw(dict, samples));
    let doc = reopen(&doc);
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    let image = xo.as_dict().unwrap().iter().next().unwrap().1.as_ref().unwrap();
    (doc, image)
}

fn assert_decode_pixels(dict: Dict, samples: Vec<u8>, expected: &[u8]) {
    let (doc, image) = decode_fixture(dict, samples);
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.images.len(), 1);
    assert_eq!(exported_png_pixels(&out.images[0].data), expected);
    let (ext, bytes) = image_file(&doc, image).unwrap();
    assert_eq!(ext, "png");
    assert_eq!(exported_png_pixels(&bytes), expected);
}

fn decode_array(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

#[test]
fn lossless_decode_maps_each_component_before_colour_conversion() {
    for (space, samples, decode, expected) in [
        ("DeviceRGB", vec![0, 64, 255], vec![1., 0., 0., 1., 0., 1.], [255, 64, 255]),
        ("DeviceRGB", vec![0, 64, 255], vec![0., 1., 1., 0., 0., 1.], [0, 191, 255]),
        ("DeviceGray", vec![0], vec![0.2, 0.8], [51, 51, 51]),
        ("DeviceGray", vec![128], vec![-1., 2.], [129, 129, 129]),
        ("DeviceCMYK", vec![0, 0, 0, 0], vec![0., 1., 1., 0., 0., 1., 0., 1.], [255, 0, 255]),
    ] {
        let mut dict = image_dict(1, 1, space, 8);
        dict.set(b"Decode".to_vec(), decode_array(&decode));
        assert_decode_pixels(dict, samples, &expected);
    }
}

#[test]
fn lossless_decode_scales_packed_samples_by_their_bit_depth() {
    let mut dict = image_dict(4, 1, "DeviceGray", 2);
    dict.set(b"Decode".to_vec(), decode_array(&[0.2, 0.8]));
    assert_decode_pixels(dict, vec![0x1b], &[51, 51, 51, 102, 102, 102, 153, 153, 153, 204, 204, 204]);
}

#[test]
fn lossless_decode_identity_and_defaults_keep_depth_and_row_padding() {
    for (bpc, samples, expected) in [
        (1, vec![0x7f, 0xff], vec![0, 0, 0, 255, 255, 255]),
        (2, vec![0x7f, 0xbf], vec![85, 85, 85, 170, 170, 170]),
        (4, vec![0x5f, 0xaf], vec![85, 85, 85, 170, 170, 170]),
        (8, vec![85, 170], vec![85, 85, 85, 170, 170, 170]),
        (16, vec![85, 255, 170, 0], vec![85, 85, 85, 170, 170, 170]),
    ] {
        for decode in [None, Some(Object::Null), Some(decode_array(&[0., 1.]))] {
            let mut dict = image_dict(1, 2, "DeviceGray", bpc);
            if let Some(decode) = decode {
                dict.set(b"Decode".to_vec(), decode);
            }
            assert_decode_pixels(dict, samples.clone(), &expected);
        }
    }
}

#[test]
fn lossless_indexed_decode_rounds_in_index_units_then_clamps() {
    let palette = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    for (bpc, samples, decode, expected) in [
        (2, vec![0x1b], Some(vec![3., 0.]), vec![100, 110, 120, 70, 80, 90, 40, 50, 60, 10, 20, 30]),
        (2, vec![0x1b], Some(vec![-1., 5.]), vec![10, 20, 30, 40, 50, 60, 100, 110, 120, 100, 110, 120]),
        (2, vec![0x1b], Some(vec![0.5, 0.5]), vec![40, 50, 60, 40, 50, 60, 40, 50, 60, 40, 50, 60]),
        (8, vec![0, 1, 2, 255], None, vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]),
        // Existing 16-bit policy retains the high byte, 42 / 255 * 3 < 0.5.
        (16, vec![0x2a, 0xab, 0, 0, 255, 255, 128, 0], Some(vec![0., 3.]), vec![10, 20, 30, 10, 20, 30, 100, 110, 120, 70, 80, 90]),
    ] {
        let (mut doc, image) = indexed_export_fixture(Object::Int(3), palette.clone(), false, false, bpc, samples);
        if let Some(decode) = decode {
            let Object::Stream(mut stream) = (*doc.get(image)).clone() else { panic!() };
            stream.dict.set(b"Decode".to_vec(), decode_array(&decode));
            doc.set(image, Object::Stream(stream));
        }
        let doc = reopen(&doc);
        let out = extract_images(&doc, &[0], 0);
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(exported_png_pixels(&out.images[0].data), expected);
        let (_, bytes) = image_file(&doc, out.images[0].object).unwrap();
        assert_eq!(exported_png_pixels(&bytes), expected);
    }
}

#[test]
fn lossless_decode_keeps_colour_key_comparisons_in_raw_samples() {
    let mut dict = image_dict(2, 1, "DeviceGray", 8);
    dict.set(b"Decode".to_vec(), decode_array(&[0.2, 0.8]));
    dict.set(b"Mask".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0)]));
    assert_decode_pixels(dict, vec![0, 255], &[51, 51, 51, 0, 204, 204, 204, 255]);

    let (mut doc, image) = indexed_export_fixture(Object::Int(1), vec![255, 0, 0, 0, 0, 255], false, false, 2, vec![0x1b]);
    let Object::Stream(mut stream) = (*doc.get(image)).clone() else { panic!() };
    stream.dict.set(b"Decode".to_vec(), decode_array(&[3., 0.]));
    stream.dict.set(b"Mask".to_vec(), Object::Array(vec![Object::Int(2), Object::Int(2)]));
    doc.set(image, Object::Stream(stream));
    let doc = reopen(&doc);
    let out = extract_images(&doc, &[0], 0);
    let expected = [0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 0, 255];
    assert_eq!(exported_png_pixels(&out.images[0].data), expected);
    let (_, bytes) = image_file(&doc, out.images[0].object).unwrap();
    assert_eq!(exported_png_pixels(&bytes), expected);
}

#[test]
fn lossless_decode_resolves_indirect_arrays_and_endpoints() {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let lo = doc.add(Object::Real(0.2));
        let hi = doc.add(Object::Real(0.8));
        let decode = doc.add(Object::Array(vec![Object::Ref(lo), Object::Ref(hi)]));
        let mut dict = image_dict(1, 1, "DeviceGray", 8);
        dict.set(b"Decode".to_vec(), Object::Ref(decode));
        Stream::from_raw(dict, vec![0])
    });
    let doc = reopen(&doc);
    let out = extract_images(&doc, &[0], 0);
    assert_eq!(exported_png_pixels(&out.images[0].data), [51, 51, 51]);
    let (_, bytes) = image_file(&doc, out.images[0].object).unwrap();
    assert_eq!(exported_png_pixels(&bytes), [51, 51, 51]);

    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let decode = doc.add(Object::Null);
        let mut dict = image_dict(1, 1, "DeviceGray", 8);
        dict.set(b"Decode".to_vec(), Object::Ref(decode));
        Stream::from_raw(dict, vec![64])
    });
    let doc = reopen(&doc);
    let out = extract_images(&doc, &[0], 0);
    assert_eq!(exported_png_pixels(&out.images[0].data), [64, 64, 64]);
    let (_, bytes) = image_file(&doc, out.images[0].object).unwrap();
    assert_eq!(exported_png_pixels(&bytes), [64, 64, 64]);
}

#[test]
fn lossless_decode_interpolation_avoids_endpoint_difference_overflow() {
    let (mut doc, image) = decode_fixture(image_dict(3, 1, "DeviceGray", 8), vec![0, 64, 255]);
    let Object::Stream(mut stream) = (*doc.get(image)).clone() else { panic!() };
    stream.dict.set(b"Decode".to_vec(), decode_array(&[-f64::MAX, f64::MAX]));
    doc.set(image, Object::Stream(stream));
    let expected = [0, 0, 0, 0, 0, 0, 255, 255, 255];
    let out = extract_images(&doc, &[0], 0);
    assert_eq!(exported_png_pixels(&out.images[0].data), expected);
    let (_, bytes) = image_file(&doc, image).unwrap();
    assert_eq!(exported_png_pixels(&bytes), expected);
}

#[test]
fn lossless_decode_falls_back_to_the_default_mapping_when_malformed() {
    for (space, samples, decode, expected) in [
        ("DeviceRGB", vec![0, 64, 255], decode_array(&[1., 0.]), [0, 64, 255]),
        ("DeviceGray", vec![64], Object::name("bad"), [64, 64, 64]),
        ("DeviceGray", vec![64], decode_array(&[]), [64, 64, 64]),
        ("DeviceGray", vec![64], decode_array(&[0.]), [64, 64, 64]),
        ("DeviceGray", vec![64], decode_array(&[1., 0., 0., 1.]), [64, 64, 64]),
        ("DeviceGray", vec![64], Object::Array(vec![Object::Int(1), Object::name("bad")]), [64, 64, 64]),
        ("DeviceGray", vec![64], Object::Array(vec![Object::Int(1), Object::Null]), [64, 64, 64]),
    ] {
        let mut dict = image_dict(1, 1, space, 8);
        dict.set(b"Decode".to_vec(), decode);
        assert_decode_pixels(dict, samples, &expected);
    }
    // Non-finite reals cannot be serialized as PDF numbers. Exercise the in-memory input.
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut dict = image_dict(1, 1, "DeviceGray", 8);
        dict.set(b"Decode".to_vec(), decode_array(&[1., value]));
        let (mut doc, image) = decode_fixture(image_dict(1, 1, "DeviceGray", 8), vec![64]);
        doc.set(image, Object::Stream(Stream::from_raw(dict, vec![64])));
        let out = extract_images(&doc, &[0], 0);
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(exported_png_pixels(&out.images[0].data), [64, 64, 64]);
        let (_, bytes) = image_file(&doc, image).unwrap();
        assert_eq!(exported_png_pixels(&bytes), [64, 64, 64]);
    }
}

/// An 8×8 baseline JPEG of one flat colour with four components, stored as `samples`, and an
/// Adobe APP14 segment with `transform` (0: CMYK, 2: YCCK).
fn flat_four_component_jpeg(samples: [u8; 4], transform: u8) -> Vec<u8> {
    fn put(bits: &mut Vec<bool>, value: u32, n: u32) {
        bits.extend((0..n).rev().map(|i| value >> i & 1 == 1));
    }
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xEE, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, transform]);
    v.extend_from_slice(&[0xFF, 0xDB, 0, 67, 0]);
    v.extend_from_slice(&[1; 64]);
    v.extend_from_slice(&[0xFF, 0xC0, 0, 20, 8, 0, 8, 0, 8, 4]);
    for id in 1..=4 {
        v.extend_from_slice(&[id, 0x11, 0]);
    }
    // DC table: categories 0 to 11 with four-bit codes equal to the category. AC table: EOB, "0".
    v.extend_from_slice(&[0xFF, 0xC4, 0, 31, 0x00, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    v.extend(0..12u8);
    v.extend_from_slice(&[0xFF, 0xC4, 0, 20, 0x10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    v.extend_from_slice(&[0xFF, 0xDA, 0, 14, 4, 1, 0, 2, 0, 3, 0, 4, 0, 0, 63, 0]);
    let mut bits = Vec::new();
    for s in samples {
        // With every quantiser 1, a flat block's DC coefficient is 8 × (sample − 128).
        let dc = (i32::from(s) - 128) * 8;
        let category = 32 - dc.unsigned_abs().leading_zeros();
        put(&mut bits, category, 4);
        put(&mut bits, if dc < 0 { (dc + (1 << category) - 1) as u32 } else { dc as u32 }, category);
        put(&mut bits, 0, 1);
    }
    bits.resize(bits.len().div_ceil(8) * 8, true);
    for byte in bits.chunks(8).map(|b| b.iter().fold(0u8, |a, &x| a << 1 | u8::from(x))) {
        v.push(byte);
        if byte == 0xFF {
            v.push(0);
        }
    }
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
fn cmyk_and_ycck_jpegs_export_in_the_colours_the_page_shows() {
    // Issue #1640: in a PDF a CMYK JPEG's samples are ink amounts, inverted only by its /Decode,
    // while JPEG decoders and image viewers read them inverted (Adobe's convention for files).
    // Exported cyan came out red. YCCK samples (Y 179, Cb 171, Cr 1) are cyan ink too.
    let inverting = Object::Array([1, 0, 1, 0, 1, 0, 1, 0].map(Object::Int).to_vec());
    for (what, samples, transform, decode, masked) in [
        ("CMYK", [255, 0, 0, 0], 0, None, true),
        ("YCCK", [179, 171, 1, 0], 2, None, true),
        ("inverted CMYK with an inverting /Decode", [0, 255, 255, 255], 0, Some(&inverting), true),
        ("CMYK without a mask", [255, 0, 0, 0], 0, None, false),
        ("inverted CMYK with an inverting /Decode, without a mask", [0, 255, 255, 255], 0, Some(&inverting), false),
    ] {
        let jpeg = flat_four_component_jpeg(samples, transform);
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let mut d = image_dict(8, 8, "DeviceCMYK", 8);
            d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
            if let Some(decode) = decode {
                d.set(b"Decode".to_vec(), decode.clone());
            }
            if masked {
                let m = doc.add(Object::Stream(Stream::from_raw(image_dict(8, 8, "DeviceGray", 8), vec![255; 64])));
                d.set(b"SMask".to_vec(), Object::Ref(m));
            }
            Stream::from_raw(d, jpeg.clone())
        });
        let (ext, rgba) = exported_rgba(&doc);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[0] <= 6 && p[1] >= 249 && p[2] >= 249), "{what}: {:?}", &rgba[..4]);
        if decode.is_some() && !masked {
            // Inverted like an image file expects: still written as is.
            assert_eq!((ext, &extract_images(&doc, &[0], 0).images[0].data), ("jpg", &jpeg), "{what}");
        }
    }
}

/// A 60 x 20 JPEG at `dpi` x 72 dpi, tagged with EXIF `orientation` (6: shown turned 90 degrees
/// clockwise).
pub(crate) fn phone_jpeg(orientation: u8, dpi: u16) -> Vec<u8> {
    use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
    let mut jpeg = Vec::new();
    let mut encoder = JpegEncoder::new(&mut jpeg);
    encoder.set_pixel_density(PixelDensity { density: (dpi, 72), unit: PixelDensityUnit::Inches });
    encoder.encode_image(&image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20]))).unwrap();
    let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xff, 0xe1]);
    out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn a_jpeg_with_an_exif_orientation_becomes_an_upright_page() {
    let doc = reopen(&from_images(&[("photo.jpg".into(), phone_jpeg(6, 144)), ("plain.jpg".into(), phone_jpeg(1, 72))]).unwrap());
    // Stored 60 x 20 px at 144 x 72 dpi; turned it is 20 x 60 px at 72 x 144 dpi: 20 x 30 pt.
    let m0 = media(&pages(&doc)[0]);
    assert!((m0[2] - 20.0).abs() < 0.01 && (m0[3] - 30.0).abs() < 0.01, "{m0:?}");
    let img = image_of(&doc, 0);
    assert_eq!((img.int(b"Width"), img.int(b"Height")), (Some(20), Some(60)));
    // "As stored" stays as it is.
    let m1 = media(&pages(&doc)[1]);
    assert!((m1[2] - 60.0).abs() < 0.01 && (m1[3] - 20.0).abs() < 0.01, "{m1:?}");
    let img = image_of(&doc, 1);
    assert_eq!((img.int(b"Width"), img.int(b"Height")), (Some(60), Some(20)));
}

#[test]
fn a_cyclic_indexed_colour_space_is_refused() {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    // 5 0 obj [/Indexed 5 0 R 255 <00>]: the base is the indexed space itself.
    let cs = doc.add(Object::Null);
    doc.set(cs, Object::Array(vec![Object::name("Indexed"), Object::Ref(cs), Object::Int(255), Object::String(PdfString::literal(vec![0]))]));
    with_image(&mut doc, |_| {
        let mut dict = image_dict(1, 1, "DeviceGray", 8);
        dict.set(b"ColorSpace".to_vec(), Object::Ref(cs));
        Stream::from_raw(dict, vec![0])
    });
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty());
    assert_eq!(out.skipped.len(), 1, "{:?}", out.skipped);
}

/// A baseline TIFF of one uncompressed strip of 1-bit samples, `packed` rows each starting on a
/// byte, 0 for black (BlackIsZero; TIFF 6.0, section 3): the `tiff` crate writes no 1-bit images.
fn one_bit_tiff(w: u32, h: u32, packed: &[u8]) -> Vec<u8> {
    // Tag, type (3 SHORT, 4 LONG), value; the strip follows the IFD.
    let entries: [(u16, u16, u32); 8] =
        [(256, 4, w), (257, 4, h), (258, 3, 1), (259, 3, 1), (262, 3, 1), (273, 4, 0), (278, 4, h), (279, 4, packed.len() as u32)];
    let strip = 8 + 2 + entries.len() as u32 * 12 + 4;
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        let value = if tag == 273 { strip } else { value };
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(1u32.to_le_bytes());
        match kind {
            3 => out.extend((value as u16).to_le_bytes().into_iter().chain([0, 0])),
            _ => out.extend(value.to_le_bytes()),
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend_from_slice(packed);
    out
}

/// A bilevel TIFF (one bit a sample) stays one bit, stored as Group 4 when that is smaller, and
/// exports as its picture.
#[test]
fn a_one_bit_tiff_is_stored_as_group_4() {
    let (w, h) = (1200u32, 240u32);
    let bits = strokes(w, h);
    let stride = (w as usize).div_ceil(8);
    let picture: Vec<bool> =
        (0..(w * h) as usize).map(|i| bits[i / w as usize * stride + i % w as usize / 8] & (0x80 >> (i % w as usize % 8)) != 0).collect();
    let doc = from_images(&[("scan.tif".into(), one_bit_tiff(w, h, &bits))]).unwrap();
    let im = image_of(&doc, 0);
    assert_eq!((im.int(b"BitsPerComponent"), im.name(b"Filter")), (Some(1), Some(&b"CCITTFaxDecode"[..])));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(exported_white(&out.images[0].data), picture);
}

/// A TIFF of `pages` 64×64 gray pages, then (with `unreadable`) one whose samples Create can't read.
fn gray_tiff(pages: usize, unreadable: bool) -> Vec<u8> {
    let mut tif = std::io::Cursor::new(Vec::new());
    let mut enc = tiff::encoder::TiffEncoder::new(&mut tif).unwrap();
    for _ in 0..pages {
        enc.write_image::<tiff::encoder::colortype::Gray8>(64, 64, &[128; 64 * 64]).unwrap();
    }
    if unreadable {
        enc.write_image::<tiff::encoder::colortype::RGB32Float>(1, 1, &[0.5; 3]).unwrap();
    }
    tif.into_inner()
}

/// Many small IFDs can each decode to a large page: the pages a TIFF keeps in memory together
/// are bounded, and embedding one image reads only its first page.
#[test]
fn tiff_pages_are_decoded_within_a_total_budget() {
    let tif = gray_tiff(3, false);
    assert!(matches!(tiff_pages_within("t.tif", &tif, usize::MAX, 10_000), Err(CreateError::Image(..))), "12 KiB of pages past a 10 KB budget");
    assert_eq!(tiff_pages_within("t.tif", &tif, usize::MAX, 20_000).unwrap().len(), 3);
    let tif = gray_tiff(1, true);
    let mut doc = Document::new_empty();
    assert!(image_xobject(&mut doc, "t.tif", &tif).is_ok(), "the unreadable last page is never decoded");
}

/// A one-pixel GIF on a `w`×`h` logical screen: the decoder paints it on a canvas that size.
fn gif_on_screen(w: u16, h: u16) -> Vec<u8> {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&w.to_le_bytes());
    gif.extend_from_slice(&h.to_le_bytes());
    gif.extend_from_slice(&[0x80, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0]);
    gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3B]);
    gif
}

/// The decoder's own limit lets a 74-byte GIF ask for a canvas of hundreds of MB, which the
/// conversion then copies; a screen past [`MAX_GIF_PIXELS`] is refused before decoding.
#[test]
fn a_tiny_gif_on_a_huge_screen_is_refused() {
    assert!(matches!(from_images(&[("big.gif".into(), gif_on_screen(4200, 4100))]), Err(CreateError::Image(..))));
    let doc = reopen(&from_images(&[("small.gif".into(), gif_on_screen(40, 30))]).unwrap());
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 40.0, 30.0]);
}

/// A PNG of `w` × `h` samples `data` in `color` at `depth`, with a palette or an ICC profile.
fn png_of(w: u32, h: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8], palette: Option<Vec<u8>>, icc: Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut info = png::Info::with_size(w, h);
        info.color_type = color;
        info.bit_depth = depth;
        info.palette = palette.map(Into::into);
        info.icc_profile = icc.map(Into::into);
        let mut writer = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
        writer.write_image_data(data).unwrap();
    }
    out
}

/// Rows of `w` one-bit samples (1 white), each on a new byte: lines of glyphs drawn from a few
/// shapes at uneven spacing, whose edges move a pixel now and then, as a scan of text has them.
/// (Clean, evenly spaced strokes favour Flate over Group 4.)
fn strokes(w: u32, h: u32) -> Vec<u8> {
    let stride = (w as usize).div_ceil(8);
    let mut data = vec![0xFF; stride * h as usize];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (w, h) = (w as usize, h as usize);
    for line in (4..h.saturating_sub(18)).step_by(24) {
        let mut x = 8;
        while x + 16 < w {
            let wide = 5 + (next() % 4) as usize;
            let shape = next() % 4;
            let mut fill = |x0: usize, x1: usize, y0: usize, y1: usize| {
                for y in line + y0..line + y1 {
                    let ragged = |r: u64| match r % 8 {
                        0 => 0,
                        1 => 2,
                        _ => 1,
                    };
                    let (left, right) = ((x + x0 + 1).saturating_sub(ragged(next())), x + x1 + ragged(next()));
                    for xx in left..right.saturating_sub(1).min(w) {
                        data[y * stride + xx / 8] &= !(0x80 >> (xx % 8));
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

/// The one-bit samples (true for white) a Group 4 image stream decodes to, through an
/// independent decoder.
fn g4_rows(s: &Stream) -> Vec<Vec<bool>> {
    struct Rows(Vec<Vec<bool>>, Vec<bool>, usize);
    impl hayro_ccitt::Decoder for Rows {
        fn push_pixel(&mut self, white: bool) {
            self.1.push(white);
        }
        fn push_pixel_chunk(&mut self, white: bool, chunks: u32) {
            self.1.extend(std::iter::repeat_n(white, chunks as usize * 8));
        }
        fn next_line(&mut self) {
            let mut row = std::mem::take(&mut self.1);
            row.truncate(self.2);
            self.0.push(row);
        }
    }
    let (w, h) = (s.dict.int(b"Width").unwrap() as u32, s.dict.int(b"Height").unwrap() as u32);
    let settings = hayro_ccitt::DecodeSettings {
        columns: w,
        rows: h,
        end_of_block: true,
        end_of_line: false,
        rows_are_byte_aligned: false,
        encoding: hayro_ccitt::EncodingMode::Group4,
        invert_black: false,
    };
    let mut out = Rows(Vec::new(), Vec::new(), w as usize);
    hayro_ccitt::decode(&s.raw, &mut out, &mut hayro_ccitt::DecoderContext::new(settings)).unwrap();
    out.0
}

fn image_stream(doc: &Document, page: usize) -> Stream {
    let p = &pages(doc)[page];
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    match &*doc.resolve(xo.get(b"Im0").unwrap()) {
        Object::Stream(s) => s.clone(),
        _ => panic!("not an image"),
    }
}

/// Black-and-white pictures are stored at one bit per pixel, as CCITT Group 4 when that is
/// smaller, whichever way the file holds them: a 1-bit PNG (which decodes to eight bits), eight-bit
/// grey of only black and white, a two-colour palette, black-and-white RGB. The bits are the
/// picture's. A grey profile stays; a picture with any other shade, colour, or an RGB profile is
/// stored as before.
#[test]
fn black_and_white_images_are_stored_at_one_bit() {
    use png::{BitDepth, ColorType};
    let (w, h) = (1200u32, 240u32);
    let bits = strokes(w, h);
    let stride = (w as usize).div_ceil(8);
    let white = |x: usize, y: usize| bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0;
    let expected: Vec<Vec<bool>> = (0..h as usize).map(|y| (0..w as usize).map(|x| white(x, y)).collect()).collect();
    let grey: Vec<u8> = expected.iter().flatten().map(|&wh| if wh { 255 } else { 0 }).collect();
    let rgb: Vec<u8> = grey.iter().flat_map(|&v| [v, v, v]).collect();
    let pictures = [
        png_of(w, h, ColorType::Grayscale, BitDepth::One, &bits, None, None),
        png_of(w, h, ColorType::Grayscale, BitDepth::Eight, &grey, None, None),
        png_of(w, h, ColorType::Indexed, BitDepth::One, &bits.iter().map(|b| !b).collect::<Vec<_>>(), Some(vec![255, 255, 255, 0, 0, 0]), None),
        png_of(w, h, ColorType::Rgb, BitDepth::Eight, &rgb, None, None),
        png_of(w, h, ColorType::Grayscale, BitDepth::One, &bits, None, Some(icc_profile(b"GRAY"))),
    ];
    let files: Vec<(String, Vec<u8>)> = pictures.into_iter().enumerate().map(|(i, p)| (format!("{i}.png"), p)).collect();
    let doc = reopen(&from_images(&files).unwrap());
    for page in 0..5 {
        let s = image_stream(&doc, page);
        assert_eq!((s.dict.int(b"BitsPerComponent"), s.dict.name(b"Filter")), (Some(1), Some(&b"CCITTFaxDecode"[..])), "page {page}");
        assert_eq!(g4_rows(&s), expected, "page {page}");
        assert!(s.raw.len() < bits.len(), "page {page}: {} bytes, smaller than its one-bit samples", s.raw.len());
    }
    assert_eq!(image_of(&doc, 3).name(b"ColorSpace"), Some(&b"DeviceGray"[..]), "black-and-white RGB becomes grey");
    assert_eq!(icc_of(&doc, &image_of(&doc, 4)).map(|(n, ..)| n), Some(1), "the grey profile stays");

    // Another shade, colour, an RGB profile: as before. A tiny picture: one bit, but Flate.
    let mut shaded = grey.clone();
    shaded[5] = 128;
    let mut coloured = rgb.clone();
    coloured[0] = 200;
    let others = [
        png_of(w, h, ColorType::Grayscale, BitDepth::Eight, &shaded, None, None),
        png_of(w, h, ColorType::Rgb, BitDepth::Eight, &coloured, None, None),
        png_of(w, h, ColorType::Rgb, BitDepth::Eight, &rgb, None, Some(icc_profile(b"RGB "))),
        png_of(2, 2, ColorType::Grayscale, BitDepth::Eight, &[0, 255, 255, 0], None, None),
    ];
    let files: Vec<(String, Vec<u8>)> = others.into_iter().enumerate().map(|(i, p)| (format!("o{i}.png"), p)).collect();
    let doc = reopen(&from_images(&files).unwrap());
    let kept: Vec<(Option<i64>, Option<Vec<u8>>)> =
        (0..4).map(|p| (image_of(&doc, p).int(b"BitsPerComponent"), image_of(&doc, p).name(b"Filter").map(<[u8]>::to_vec))).collect();
    assert_eq!(
        kept[..3],
        [(Some(8), Some(b"FlateDecode".to_vec())), (Some(8), Some(b"FlateDecode".to_vec())), (Some(8), Some(b"FlateDecode".to_vec()))]
    );
    assert_eq!(kept[3], (Some(1), Some(b"FlateDecode".to_vec())), "Group 4 isn't smaller for 2 × 2");
}

/// The pixels of an exported PNG (RGB), as true for white.
fn exported_white(png_data: &[u8]) -> Vec<bool> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_data.to_vec())).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Rgb);
    buf[..frame.buffer_size()].as_chunks::<3>().0.iter().map(|p| *p == [255, 255, 255]).collect()
}

/// Export ▸ Images reads CCITT fax images: a page created from a 1-bit PNG (Group 4 now) exports as
/// the picture, as does the picture coded the other way round with /BlackIs1, the picture with its
/// filter and parameters in arrays, with /K and /Columns given by reference, with a /Rows below its
/// height, and behind another filter (its parameters by reference too). One whose /Columns aren't
/// its width is skipped, not misread.
#[test]
fn ccitt_fax_images_export_as_their_picture() {
    use png::{BitDepth, ColorType};
    let (w, h) = (1200u32, 240u32);
    let bits = strokes(w, h);
    let stride = (w as usize).div_ceil(8);
    let picture: Vec<bool> =
        (0..h as usize).flat_map(|y| (0..w as usize).map(move |x| (y, x))).map(|(y, x)| bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0).collect();
    let mut doc = from_images(&[("scan.png".into(), png_of(w, h, ColorType::Grayscale, BitDepth::One, &bits, None, None))]).unwrap();
    assert_eq!(image_of(&doc, 0).name(b"Filter"), Some(&b"CCITTFaxDecode"[..]));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(exported_white(&out.images[0].data), picture);

    // The same picture coded with 1 for black, which /BlackIs1 says.
    let r = {
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        xo.get(b"Im0").and_then(Object::as_ref).unwrap()
    };
    let base = image_of(&doc, 0);
    let with = |black_is_1: bool, columns: i64, data: Vec<u8>| {
        let mut d = base.clone();
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(-1));
        parms.set(b"Columns".to_vec(), Object::Int(columns));
        parms.set(b"Rows".to_vec(), Object::Int(i64::from(h)));
        parms.set(b"BlackIs1".to_vec(), Object::Bool(black_is_1));
        d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        d.remove(b"Length");
        Stream::from_raw(d, data)
    };
    let inverted: Vec<u8> = bits.iter().map(|b| !b).collect();
    let stream = with(true, i64::from(w), pdfcraft_filters::encode_g4(&inverted, w, h).unwrap());
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert_eq!(exported_white(&out.images[0].data), picture, "BlackIs1");

    let mut stream = with(false, i64::from(w), pdfcraft_filters::encode_g4(&bits, w, h).unwrap());
    let parms = stream.dict.get(b"DecodeParms").cloned().unwrap();
    stream.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![parms]));
    stream.dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("CCITTFaxDecode")]));
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(exported_white(&out.images[0].data), picture, "in arrays");

    let mut stream = with(false, i64::from(w), pdfcraft_filters::encode_g4(&bits, w, h).unwrap());
    let mut parms = stream.dict.get(b"DecodeParms").and_then(Object::as_dict).cloned().unwrap();
    parms.set(b"K".to_vec(), Object::Ref(doc.add(Object::Int(-1))));
    parms.set(b"Columns".to_vec(), Object::Ref(doc.add(Object::Int(i64::from(w)))));
    parms.set(b"Rows".to_vec(), Object::Int(1));
    stream.dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(exported_white(&out.images[0].data), picture, "/K and /Columns by reference, /Rows 1");

    // Behind another filter, with its parameters second in the array, as 7.4.1 pairs them.
    let g4 = pdfcraft_filters::encode_g4(&bits, w, h).unwrap();
    let hex = g4.iter().map(|b| format!("{b:02x}")).chain([">".to_string()]).collect::<String>().into_bytes();
    for (outer, data) in [("ASCIIHexDecode", hex), ("FlateDecode", pdfcraft_filters::encode_flate(&g4))] {
        let mut stream = with(false, i64::from(w), data);
        let parms = stream.dict.get(b"DecodeParms").cloned().unwrap();
        stream.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Null, parms]));
        stream.dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name(outer), Object::name("CCITTFaxDecode")]));
        doc.set(r, Object::Stream(stream));
        let out = extract_images(&doc, &[0], 0);
        assert!(out.skipped.is_empty(), "{outer}: {:?}", out.skipped);
        assert_eq!(exported_white(&out.images[0].data), picture, "behind {outer}");
    }
    // Behind Flate with a PNG predictor, whose parameters are given by reference.
    let padded: Vec<u8> = g4.iter().copied().chain(std::iter::repeat_n(0, 16 - g4.len() % 16)).collect();
    let params = pdfcraft_filters::Params { predictor: 12, colors: 1, bits_per_component: 8, columns: 16, early_change: 1 };
    let predicted = pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, &padded).unwrap();
    let mut flate_parms = Dict::new();
    flate_parms.set(b"Predictor".to_vec(), Object::Int(12));
    flate_parms.set(b"Columns".to_vec(), Object::Int(16));
    let flate_parms = doc.add(Object::Dict(flate_parms));
    let mut stream = with(false, i64::from(w), predicted);
    let parms = stream.dict.get(b"DecodeParms").cloned().unwrap();
    stream.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Ref(flate_parms), parms]));
    stream.dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode"), Object::name("CCITTFaxDecode")]));
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(exported_white(&out.images[0].data), picture, "behind Flate with a predictor given by reference");
    // CCITT before another filter isn't a picture yet.
    let mut stream = with(false, i64::from(w), pdfcraft_filters::encode_flate(&g4));
    stream.dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("CCITTFaxDecode"), Object::name("FlateDecode")]));
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty() && out.skipped.len() == 1, "CCITT, then Flate: skipped");

    let stream = with(false, i64::from(w) + 8, pdfcraft_filters::encode_g4(&bits, w, h).unwrap());
    doc.set(r, Object::Stream(stream));
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty() && out.skipped.len() == 1, "columns other than the width: skipped");
}

/// Group 3 data: rows of run lengths below 64, white first, coded one-dimensionally with T.4's
/// terminating codes (ITU-T T.4 Tables 2 and 3). Each row starts with an end-of-line code with
/// `eol`, then with `tag` a 1 bit (the next row one-dimensional, as `/K` above 0 has it). Fill bits
/// (0s) go where `fill` says.
fn group_3(rows: &[impl AsRef<[u32]>], eol: bool, tag: bool, fill: Fill) -> Vec<u8> {
    // The codes for runs 0 to 63, in order.
    const WHITE: &str = concat!(
        "00110101 000111 0111 1000 1011 1100 1110 1111 10011 10100 00111 01000 001000 000011 110100 110101 101010 101011 ",
        "0100111 0001100 0001000 0010111 0000011 0000100 0101000 0101011 0010011 0100100 0011000 00000010 00000011 00011010 ",
        "00011011 00010010 00010011 00010100 00010101 00010110 00010111 00101000 00101001 00101010 00101011 00101100 00101101 ",
        "00000100 00000101 00001010 00001011 01010010 01010011 01010100 01010101 00100100 00100101 01011000 01011001 01011010 ",
        "01011011 01001010 01001011 00110010 00110011 00110100",
    );
    const BLACK: &str = concat!(
        "0000110111 010 11 10 011 0011 0010 00011 000101 000100 0000100 0000101 0000111 00000100 00000111 000011000 0000010111 ",
        "0000011000 0000001000 00001100111 00001101000 00001101100 00000110111 00000101000 00000010111 00000011000 ",
        "000011001010 000011001011 000011001100 000011001101 000001101000 000001101001 000001101010 000001101011 000011010010 ",
        "000011010011 000011010100 000011010101 000011010110 000011010111 000001101100 000001101101 000011011010 000011011011 ",
        "000001010100 000001010101 000001010110 000001010111 000001100100 000001100101 000001010010 000001010011 000000100100 ",
        "000000110111 000000111000 000000100111 000000101000 000001011000 000001011001 000000101011 000000101100 000001011010 ",
        "000001100110 000001100111",
    );
    let code = |white: bool, run: u32| (if white { WHITE } else { BLACK }).split(' ').nth(run as usize).expect("a run below 64");
    let mut bits = String::new();
    for row in rows {
        // The end-of-line code is 12 bits.
        let ends_code = if fill == Fill::CodeEnd && eol { 12 } else { 0 };
        while fill != Fill::None && !(bits.len() + ends_code).is_multiple_of(8) {
            bits.push('0');
        }
        if eol {
            bits.push_str("000000000001");
        }
        if tag {
            bits.push('1');
        }
        row.as_ref().iter().enumerate().for_each(|(i, run)| bits.push_str(code(i % 2 == 0, *run)));
    }
    bits.as_bytes().chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, d)| b | (u8::from(*d == b'1') << (7 - i)))).collect()
}

/// `count` text-like pictures `w` by `h` for [`group_3`], from a fixed seed: rows of random runs,
/// long white ones and short black ones, and a row may start black.
fn random_pictures(w: u32, h: u32, count: usize) -> Vec<Vec<Vec<u32>>> {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n) as u32
    };
    let mut row = || {
        let (mut runs, mut left) = (Vec::new(), w);
        while left > 0 {
            let run = match runs.len() % 2 {
                0 if runs.is_empty() => random(31),
                0 => 1 + random(30),
                _ => 1 + random(5),
            };
            runs.push(run.min(left));
            left -= run.min(left);
        }
        runs
    };
    (0..count).map(|_| (0..h).map(|_| row()).collect()).collect()
}

/// The picture `rows` of runs (white first) make, `w` by `h`, true where white.
fn white_of(rows: &[Vec<u32>], w: u32, h: u32) -> Vec<bool> {
    let mut white = vec![false; (w * h) as usize];
    for (y, row) in rows.iter().enumerate() {
        let mut x = y * w as usize;
        for (i, run) in row.iter().enumerate() {
            white[x..x + *run as usize].fill(i % 2 == 0);
            x += *run as usize;
        }
    }
    white
}

/// Where [`group_3`] puts fill bits: none; so each row, its end-of-line code first, starts on a
/// byte; or so each end-of-line code ends on one (T.4's fill, as TIFF writes it).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fill {
    None,
    RowStart,
    CodeEnd,
}

/// What Export ▸ Images claims for CCITT and for parameters given by reference, each checked
/// against the same picture stored another way: Group 3 one-dimensional data (`/K 0`) exports as
/// its picture; a CCITT soft mask and a CCITT stencil `/Mask` export as the same masks in Flate;
/// and a Flate image's PNG predictor, and an indexed image's lookup table stream with one, read
/// the same with their `/DecodeParms` given by reference as direct.
#[test]
fn group_3_ccitt_masks_and_parameters_by_reference_export_as_their_picture() {
    use png::{BitDepth, ColorType};
    let (w, h) = (64u32, 16u32);
    let rows: Vec<&[u32]> = (0..h).map(|y| if y % 2 == 0 { &[32u32, 32][..] } else { &[16, 16, 32][..] }).collect();
    let mut bits = vec![0u8; 8 * h as usize];
    for (y, runs) in rows.iter().enumerate() {
        let mut x = 0;
        for (i, run) in runs.iter().enumerate() {
            if i % 2 == 0 {
                (x..x + *run as usize).for_each(|xx| bits[y * 8 + xx / 8] |= 0x80 >> (xx % 8));
            }
            x += *run as usize;
        }
    }
    let picture: Vec<bool> = (0..(w * h) as usize).map(|i| bits[i / 8] & (0x80 >> (i % 8)) != 0).collect();
    let mut doc = from_images(&[("scan.png".into(), png_of(w, h, ColorType::Grayscale, BitDepth::One, &bits, None, None))]).unwrap();
    let r = {
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        xo.get(b"Im0").and_then(Object::as_ref).unwrap()
    };
    let image = |entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w.into()));
        d.set(b"Height".to_vec(), Object::Int(h.into()));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        d
    };
    let fax = |k: i64| {
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(k));
        parms.set(b"Columns".to_vec(), Object::Int(w.into()));
        parms.set(b"Rows".to_vec(), Object::Int(h.into()));
        [("Filter", Object::name("CCITTFaxDecode")), ("DecodeParms", Object::Dict(parms))]
    };
    let grey = |bits: i64| [("BitsPerComponent", Object::Int(bits)), ("ColorSpace", Object::name("DeviceGray"))];

    // One-dimensional (/K 0) plain, with end-of-line codes, with each row on a byte, and with
    // both; mixed (/K 1, each row tagged one-dimensional) with end-of-line codes and without.
    // Byte-aligned with end-of-line codes either way: each code starting on a byte, or ending on
    // one (T.4's fill, as TIFF writes it). /EncodedByteAlign is given by reference once, and so is
    // a /BlackIs1 that inverts the picture.
    let inverted: Vec<bool> = picture.iter().map(|white| !white).collect();
    for (k, eol, fill, align_by_reference, black_is_1) in [
        (0, false, Fill::None, false, false),
        (0, true, Fill::None, false, false),
        (0, false, Fill::RowStart, true, false),
        (0, true, Fill::RowStart, false, false),
        (0, true, Fill::CodeEnd, false, false),
        (1, true, Fill::None, false, false),
        (1, false, Fill::None, false, false),
        (1, true, Fill::CodeEnd, false, false),
        (0, false, Fill::None, false, true),
    ] {
        let mut d = image(&[&grey(1)[..], &fax(k)].concat());
        let align = Object::Bool(fill != Fill::None);
        let align = if align_by_reference { Object::Ref(doc.add(align)) } else { align };
        let inverts = Object::Ref(doc.add(Object::Bool(black_is_1)));
        if let Some(Object::Dict(parms)) = d.get_mut(b"DecodeParms") {
            parms.set(b"EndOfLine".to_vec(), Object::Bool(eol));
            parms.set(b"EncodedByteAlign".to_vec(), align);
            parms.set(b"BlackIs1".to_vec(), inverts);
        }
        doc.set(r, Object::Stream(Stream::from_raw(d, group_3(&rows, eol, k > 0, fill))));
        let case = format!("/K {k}, EndOfLine {eol}, fill {fill:?}, BlackIs1 {black_is_1}");
        let out = extract_images(&doc, &[0], 0);
        assert!(out.skipped.is_empty(), "{case}: {:?}", out.skipped);
        assert_eq!(exported_white(&out.images[0].data), if black_is_1 { inverted.clone() } else { picture.clone() }, "{case}");
    }
    // Byte-aligned rows against /EndOfLine, over several pictures, as how a misreading comes out
    // depends on the picture: no codes (as TIFF's Modified Huffman has it), and /EndOfLine without
    // them; codes, each starting on a byte or ending on one, /EndOfLine or not. Each exports as its
    // picture, and a byte short is reported. The first picture is stripes, every row 32 white then
    // 32 black; the others are text-like rows of random runs.
    let image_with = |doc: &mut Document, k: i64, eol: bool, data: Vec<u8>| {
        let mut d = image(&[&grey(1)[..], &fax(k)].concat());
        if let Some(Object::Dict(parms)) = d.get_mut(b"DecodeParms") {
            parms.set(b"EndOfLine".to_vec(), Object::Bool(eol));
            parms.set(b"EncodedByteAlign".to_vec(), Object::Bool(true));
        }
        doc.set(r, Object::Stream(Stream::from_raw(d, data)));
        extract_images(doc, &[0], 0)
    };
    let mut pictures = vec![vec![vec![32u32, 32]; h as usize]];
    pictures.extend(random_pictures(w, h, 8));
    for (n, runs) in pictures.iter().enumerate() {
        let white = white_of(runs, w, h);
        for k in [0, 1] {
            for (codes, fill, eol) in [
                (false, Fill::RowStart, false),
                (false, Fill::RowStart, true),
                (true, Fill::RowStart, false),
                (true, Fill::RowStart, true),
                (true, Fill::CodeEnd, false),
                (true, Fill::CodeEnd, true),
            ] {
                let out = image_with(&mut doc, k, eol, group_3(runs, codes, k > 0, fill));
                let case = format!("picture {n}, /K {k}, codes {codes}, fill {fill:?}, /EndOfLine {eol}");
                assert!(out.skipped.is_empty(), "{case}: {:?}", out.skipped);
                assert_eq!(exported_white(&out.images[0].data), white, "{case}");
                // A byte short, it is reported, not read the other way into other rows.
                let mut short = group_3(runs, codes, k > 0, fill);
                short.pop();
                let out = image_with(&mut doc, k, eol, short);
                assert!(out.images.is_empty() && out.skipped.len() == 1, "{case}, a byte short: {:?}", out.skipped);
                // At /K 1, codes with none before the first row (the first code and its fill are 2
                // bytes here) are read on bytes: the picture or reported, never another picture.
                if k == 1 && fill == Fill::CodeEnd {
                    let out = image_with(&mut doc, k, eol, group_3(runs, codes, true, fill).split_off(2));
                    assert!(out.images.iter().all(|i| exported_white(&i.data) == white), "{case}, no code first");
                }
            }
        }
    }
    // Group 4 rows on bytes keep their aligned read, /EndOfLine or not: here white rows, each the
    // one code 1 (vertical, no change) and its fill.
    for eol in [false, true] {
        let out = image_with(&mut doc, -1, eol, vec![0x80; h as usize]);
        assert!(out.skipped.is_empty(), "byte-aligned Group 4, /EndOfLine {eol}: {:?}", out.skipped);
        assert!(exported_white(&out.images[0].data).iter().all(|white| *white), "byte-aligned Group 4, /EndOfLine {eol}");
    }

    // A grey picture under the same bits as a soft mask, then as a stencil mask: CCITT against Flate.
    let g4 = pdfcraft_filters::encode_g4(&bits, w, h).unwrap();
    for (key, mask) in [("SMask", grey(1).to_vec()), ("Mask", vec![("ImageMask", Object::Bool(true))])] {
        let mut exported = Vec::new();
        for coded in [true, false] {
            let m = if coded { Stream::from_raw(image(&[&mask[..], &fax(-1)].concat()), g4.clone()) } else { Stream::flate(image(&mask), &bits) };
            let m = doc.add(Object::Stream(m));
            let d = image(&[&grey(8)[..], &[(key, Object::Ref(m))]].concat());
            doc.set(r, Object::Stream(Stream::flate(d, &vec![100u8; (w * h) as usize])));
            exported.push(exported_rgba(&doc));
        }
        assert!(exported[0].1.chunks(4).any(|p| p[3] == 0), "{key}: masked somewhere");
        assert_eq!(exported[0], exported[1], "{key}: CCITT as Flate");
    }

    // A PNG predictor given by reference, for the samples and for a lookup table.
    let predicted = |rows: &[u8], columns: i64| {
        let params = pdfcraft_filters::Params { predictor: 12, colors: 1, bits_per_component: 8, columns, early_change: 1 };
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), Object::Int(12));
        parms.set(b"Columns".to_vec(), Object::Int(columns));
        (pdfcraft_filters::encode(&pdfcraft_filters::Filter::Flate, &params, rows).unwrap(), parms)
    };
    let shades: Vec<u8> = (0..w * h).map(|i| (i * 7 % 256) as u8).collect();
    let (table, table_parms) = predicted(&[255, 0, 0, 0, 0, 255], 3);
    let indices: Vec<u8> = (0..w * h).map(|i| (i % 3 % 2) as u8).collect();
    let mut exported = Vec::new();
    for by_reference in [false, true] {
        let parms = |doc: &mut Document, p: &Dict| if by_reference { Object::Ref(doc.add(Object::Dict(p.clone()))) } else { Object::Dict(p.clone()) };
        let (samples, sample_parms) = predicted(&shades, w.into());
        let sample_parms = parms(&mut doc, &sample_parms);
        let d = image(&[&grey(8)[..], &[("Filter", Object::name("FlateDecode")), ("DecodeParms", sample_parms)]].concat());
        doc.set(r, Object::Stream(Stream::from_raw(d, samples)));
        let plain = exported_rgba(&doc);

        let mut lookup = Dict::new();
        lookup.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        lookup.set(b"DecodeParms".to_vec(), parms(&mut doc, &table_parms));
        let lookup = doc.add(Object::Stream(Stream::from_raw(lookup, table.clone())));
        let cs = Object::Array(vec![Object::name("Indexed"), Object::name("DeviceRGB"), 1.into(), Object::Ref(lookup)]);
        let d = image(&[("BitsPerComponent", Object::Int(8)), ("ColorSpace", cs)]);
        doc.set(r, Object::Stream(Stream::flate(d, &indices)));
        exported.push((plain, exported_rgba(&doc)));
    }
    assert_eq!(exported[0].0.1.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), shades, "the samples, direct");
    assert_eq!(exported[0].1.1.get(..8), Some(&[255, 0, 0, 255, 0, 0, 255, 255][..]), "red, then blue");
    assert_eq!(exported[1], exported[0], "by reference as direct");
}

/// An image whose PNG predictor is given by reference exports as its picture: its `/DecodeParms`
/// dictionary given by reference, as an array's entry, or direct with its numbers by reference. So
/// do an indexed image whose lookup table is a stream with its `/DecodeParms` given by reference,
/// and a JBIG2 image whose `/JBIG2Globals` stream has them so. `Stream::decoded` reads only a
/// direct `/DecodeParms` with direct numbers: the row tags stayed in the samples, and the picture
/// came out shifted.
#[test]
fn images_whose_predictor_is_given_by_reference_export_as_their_picture() {
    // PNG predictor parameters for `columns` one-byte samples a row.
    let parms = |columns: i64| {
        let mut p = Dict::new();
        p.set(b"Predictor".to_vec(), Object::Int(15));
        p.set(b"Colors".to_vec(), Object::Int(1));
        p.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        p.set(b"Columns".to_vec(), Object::Int(columns));
        p
    };
    // Two rows of four grey samples, each behind a PNG "None" row tag.
    let predicted = pdfcraft_filters::encode_flate(&[0, 10, 20, 30, 40, 0, 50, 60, 70, 80]);
    let grey: Vec<u8> = [10u8, 20, 30, 40, 50, 60, 70, 80].iter().flat_map(|v| [*v, *v, *v, 255]).collect();
    for form in ["a reference", "an array of a reference", "numbers by reference"] {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let mut d = image_dict(4, 2, "DeviceGray", 8);
            d.set(b"Filter".to_vec(), Object::name("FlateDecode"));
            let decode_parms = match form {
                "numbers by reference" => {
                    let mut p = parms(4);
                    for key in [&b"Predictor"[..], b"Columns"] {
                        let v = p.get(key).cloned().unwrap();
                        p.set(key.to_vec(), Object::Ref(doc.add(v)));
                    }
                    Object::Dict(p)
                }
                "a reference" => Object::Ref(doc.add(Object::Dict(parms(4)))),
                _ => Object::Array(vec![Object::Ref(doc.add(Object::Dict(parms(4))))]),
            };
            d.set(b"DecodeParms".to_vec(), decode_parms);
            Stream::from_raw(d, predicted.clone())
        });
        assert_eq!(exported_rgba(&doc), ("png", grey.clone()), "/DecodeParms as {form}");
    }
    // An indexed image's lookup table, red then blue, as a stream behind a "None" row tag.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut t = Dict::new();
        t.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        t.set(b"DecodeParms".to_vec(), Object::Ref(doc.add(Object::Dict(parms(6)))));
        let table = doc.add(Object::Stream(Stream::from_raw(t, pdfcraft_filters::encode_flate(&[0, 255, 0, 0, 0, 0, 255]))));
        let mut d = image_dict(4, 1, "DeviceGray", 8);
        d.set(b"ColorSpace".to_vec(), Object::Array(vec![Object::name("Indexed"), Object::name("DeviceRGB"), Object::Int(1), Object::Ref(table)]));
        Stream::from_raw(d, vec![0, 1, 0, 1])
    });
    let (red, blue) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    assert_eq!(exported_rgba(&doc), ("png", [red, blue, red, blue].concat()), "the lookup table's /DecodeParms by reference");
    // A JBIG2 image whose globals hold one extension segment (ISO/IEC 14492 7.4.14) a decoder
    // ignores, behind a "None" row tag. With the tag left in, the globals can't be read and the
    // image is skipped.
    let segment = [0, 0, 0, 0, 0x3E, 0, 0, 0, 0, 0, 4, 0, 0, 0, 1];
    let bits = [0xF0u8, 0x0F, 0xAA, 0x55];
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut g = Dict::new();
        g.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        g.set(b"DecodeParms".to_vec(), Object::Ref(doc.add(Object::Dict(parms(segment.len() as i64)))));
        let tagged: Vec<u8> = [0].iter().chain(&segment).copied().collect();
        let globals = doc.add(Object::Stream(Stream::from_raw(g, pdfcraft_filters::encode_flate(&tagged))));
        let mut decode_parms = Dict::new();
        decode_parms.set(b"JBIG2Globals".to_vec(), Object::Ref(globals));
        let mut d = image_dict(16, 2, "DeviceGray", 1);
        d.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
        d.set(b"DecodeParms".to_vec(), Object::Dict(decode_parms));
        Stream::from_raw(d, pdfcraft_filters::encode_jbig2_generic(&bits, 16, 2).unwrap())
    });
    let picture: Vec<u8> = (0..32).flat_map(|i| if bits[i / 8] & (0x80 >> (i % 8)) != 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
    assert_eq!(exported_rgba(&doc), ("png", picture), "the JBIG2 globals' /DecodeParms by reference");
}

/// An image the optimizer stored as JBIG2 symbols (a symbol dictionary and text regions, rather
/// than one generic region) exports as its picture.
#[test]
fn symbol_coded_jbig2_images_export_as_their_picture() {
    const LETTERS: [[&str; 7]; 3] = [
        ["#####", "#....", "####.", "#....", "#....", "#....", "#####"],
        [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
        ["#...#", "#...#", "#####", "#...#", "#...#", "#...#", "#...#"],
    ];
    let (w, h) = (400usize, 40usize);
    let stride = w.div_ceil(8);
    // 1 samples white, as a grey image's are; the letters' pixels 0.
    let mut bits = vec![0xFFu8; stride * h];
    for (line, top) in [2usize, 12, 22, 32].into_iter().enumerate() {
        for (k, x) in (3..w - 6).step_by(7).enumerate() {
            for (dy, row) in LETTERS[(k + line) % 3].iter().enumerate() {
                for (dx, c) in row.bytes().enumerate() {
                    if c == b'#' {
                        bits[(top + dy) * stride + (x + dx) / 8] &= !(0x80 >> ((x + dx) % 8));
                    }
                }
            }
        }
    }
    let jbig2 = pdfcraft_filters::encode_jbig2(&bits, w as u32, h as u32).unwrap();
    let generic = pdfcraft_filters::encode_jbig2_generic(&bits, w as u32, h as u32).unwrap();
    assert!(jbig2.len() < generic.len(), "symbol coded: {} bytes, generic {}", jbig2.len(), generic.len());
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| {
        let mut d = image_dict(w as i64, h as i64, "DeviceGray", 1);
        d.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
        Stream::from_raw(d, jbig2)
    });
    let picture: Vec<u8> = (0..w * h)
        .flat_map(|i| if bits[i / w * stride + i % w / 8] & (0x80 >> (i % w % 8)) != 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] })
        .collect();
    assert_eq!(exported_rgba(&doc), ("png", picture));
}

/// Export ▸ Images reads JBIG2 images (ISO 32000-2 §7.4.7), JBIG2 black as 0: as the image, and as
/// a stencil `/Mask` of another. Their `/JBIG2Globals` are read, from a dictionary or the one in
/// an array, so globals that can't be read skip the image rather than being ignored. Rows past the
/// image's height are left out; a JBIG2 page of another width, or with fewer rows than the image,
/// or larger than an exported image may be (however small the image's `/Height`), or data that
/// isn't JBIG2, skips the image.
#[test]
fn jbig2_images_export_as_their_picture() {
    let (w, h) = (203i64, 41i64);
    let stride = 26;
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let bits: Vec<u8> = (0..stride * h as usize)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    // The first `rows` rows as RGBA: a 1 sample white, a 0 black.
    let picture = |rows: i64| -> Vec<u8> {
        (0..rows as usize)
            .flat_map(|y| (0..w as usize).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let v = if bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0 { 255 } else { 0 };
                [v, v, v, 255]
            })
            .collect()
    };
    let jbig2 = pdfcraft_filters::encode_jbig2_generic(&bits, w as u32, h as u32).unwrap();
    // A page whose image is `data` as JBIG2, `width` × `height`. With `globals`, its /DecodeParms
    // (an array when `.1`) name a globals stream; with `mask`, it is the stencil /Mask of a grey
    // image.
    let page = |width: i64, height: i64, data: &[u8], globals: Option<(&[u8], bool)>, mask: bool| {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let mut d = image_dict(width, height, "DeviceGray", 1);
            d.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
            if let Some((g, in_array)) = globals {
                let g = doc.add(Object::Stream(Stream::from_raw(Dict::new(), g.to_vec())));
                let mut parms = Dict::new();
                parms.set(b"JBIG2Globals".to_vec(), Object::Ref(g));
                d.set(b"DecodeParms".to_vec(), if in_array { Object::Array(vec![Object::Dict(parms)]) } else { Object::Dict(parms) });
            }
            if !mask {
                return Stream::from_raw(d, data.to_vec());
            }
            d.remove(b"ColorSpace");
            d.set(b"ImageMask".to_vec(), Object::Bool(true));
            let m = doc.add(Object::Stream(Stream::from_raw(d, data.to_vec())));
            let mut image = image_dict(width, height, "DeviceGray", 8);
            image.set(b"Mask".to_vec(), Object::Ref(m));
            Stream::from_raw(image, vec![128; (width * height) as usize])
        });
        doc
    };
    let skipped = |doc: &Document| {
        let out = extract_images(doc, &[0], 0);
        out.images.is_empty() && out.skipped.len() == 1
    };

    assert_eq!(exported_rgba(&page(w, h, &jbig2, None, false)), ("png", picture(h)));
    assert_eq!(exported_rgba(&page(w, h, &jbig2, Some((&[][..], false)), false)).1, picture(h), "empty globals");
    for in_array in [false, true] {
        assert!(skipped(&page(w, h, &jbig2, Some((&b"not JBIG2"[..], in_array)), false)), "unreadable globals, in an array: {in_array}");
    }
    assert_eq!(exported_rgba(&page(w, h - 5, &jbig2, None, false)).1, picture(h - 5), "rows past the height");
    for (width, height, data) in [(w + 8, h, &jbig2[..]), (w - 1, h, &jbig2[..]), (w, h + 1, &jbig2[..]), (w, h, &b"not JBIG2"[..])] {
        assert!(skipped(&page(width, height, data, None, false)), "{width} × {height}, {} bytes", data.len());
    }
    // The same stream with its page information (the first segment's data, after its 11-byte
    // header: width, then height) claiming a 20,000 × 10,000 page: 200 million pixels, over the
    // export's 100 million, behind an image of 20,000 × 4. The decoder would decode the whole page
    // (it takes pages of up to about 2^28 pixels), so it is refused first.
    let mut large = jbig2.clone();
    large[11..15].copy_from_slice(&20_000u32.to_be_bytes());
    large[15..19].copy_from_slice(&10_000u32.to_be_bytes());
    assert!(skipped(&page(20_000, 4, &large, None, false)), "a page over the export's size");
    // Refused the same ways as a stencil /Mask, it is left out: the image exports opaque.
    for (width, height, mask) in [(w + 8, h, &jbig2), (20_000, 4, &large)] {
        let rgba = exported_rgba(&page(width, height, mask, None, true)).1;
        assert_eq!(rgba.len() as i64, width * height * 4, "a {width}-wide mask");
        assert!(alphas(&rgba).iter().all(|a| *a == 255), "a refused {width}-wide mask is left out");
    }
    // A page of exactly the budget, 20,000 × 5,000, is decoded; one row more is refused.
    for (rows, refused) in [(5_000u32, false), (5_001, true)] {
        large[15..19].copy_from_slice(&rows.to_be_bytes());
        let doc = page(20_000, 4, &large, None, false);
        assert_eq!(skipped(&doc), refused, "a page of 20,000 × {rows}");
        if !refused {
            assert_eq!(exported_rgba(&doc).1.len(), 20_000 * 4 * 4);
        }
    }
    // Damaged data never panics.
    for end in [0, 1, 10, 20, 30, jbig2.len() / 2] {
        let _ = extract_images(&page(w, h, &jbig2[..end], None, false), &[0], 0);
    }

    // As a stencil /Mask, its 1 samples (JBIG2 white) hide the image.
    let alpha: Vec<u8> = picture(h).as_chunks::<4>().0.iter().map(|p| 255 - p[0]).collect();
    assert_eq!(alphas(&exported_rgba(&page(w, h, &jbig2, None, true)).1), alpha);
}

/// A JBIG2 image's `/JBIG2Globals` are decoded before its JBIG2 page's size is known, so they have
/// a limit of their own, 32 MiB, far below the 1 GiB of any stream: globals that Flate-decode to
/// one byte more skip the image, with the limit in the reason, though they take a few kilobytes
/// in the file. Globals of exactly the limit are read, and the limit is the same through an
/// indirect `/DecodeParms`, in an array or not.
#[test]
fn jbig2_globals_are_decoded_within_a_limit_of_their_own() {
    const LIMIT: usize = 32 << 20;
    let bits = [0xF0u8, 0x0F, 0xAA, 0x55];
    let jbig2 = pdfcraft_filters::encode_jbig2_generic(&bits, 16, 2).unwrap();
    let picture: Vec<u8> = (0..32).flat_map(|i| if bits[i / 8] & (0x80 >> (i % 8)) != 0 { [255; 4] } else { [0, 0, 0, 255] }).collect();
    // Globals of `len` bytes, Flate-encoded: one extension segment (ISO/IEC 14492 7.4.14), which a
    // decoder skips; its 11-byte header gives the length of the data after it.
    let globals = |len: usize| {
        let mut g = vec![0, 0, 0, 0, 0x3E, 0, 0];
        g.extend_from_slice(&u32::try_from(len - 11).unwrap().to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 1]);
        g.resize(len, 0);
        pdfcraft_filters::encode_flate(&g)
    };
    // The image, its /DecodeParms naming `flated` as its globals: direct (0), by reference (1), or
    // by reference in an array (2).
    let page = |flated: &[u8], form: usize| {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let mut g = Dict::new();
            g.set(b"Filter".to_vec(), Object::name("FlateDecode"));
            let g = doc.add(Object::Stream(Stream::from_raw(g, flated.to_vec())));
            let mut parms = Dict::new();
            parms.set(b"JBIG2Globals".to_vec(), Object::Ref(g));
            let parms = match form {
                0 => Object::Dict(parms),
                1 => Object::Ref(doc.add(Object::Dict(parms))),
                _ => Object::Array(vec![Object::Ref(doc.add(Object::Dict(parms)))]),
            };
            let mut d = image_dict(16, 2, "DeviceGray", 1);
            d.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
            d.set(b"DecodeParms".to_vec(), parms);
            Stream::from_raw(d, jbig2.clone())
        });
        doc
    };

    let (small, full, over) = (globals(15), globals(LIMIT), globals(LIMIT + 1));
    assert!(over.len() < 64 << 10, "the oversized globals take {} bytes in the file", over.len());
    assert_eq!(exported_rgba(&page(&full, 0)).1, picture, "globals of exactly the limit");
    for form in 0..3 {
        assert_eq!(exported_rgba(&page(&small, form)).1, picture, "small globals, form {form}");
        let out = extract_images(&page(&over, form), &[0], 0);
        assert!(out.images.is_empty(), "globals over the limit, form {form}");
        let why = &out.skipped.first().expect("the image is skipped").2;
        assert!(why.starts_with("the JBIG2 globals can't be read") && why.contains(&format!("limit of {LIMIT} bytes")), "{why}");
    }
}
