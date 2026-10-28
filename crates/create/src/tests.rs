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
