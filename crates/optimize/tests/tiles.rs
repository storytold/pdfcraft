//! Large pictures drawn as tiles for Windows printing (#5131): the page looks the same, and no
//! image it draws is over the size Windows' renderer shrinks.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::{DOWNSAMPLED_ABOVE, TileReport, tile_large_images};

/// 4200 × 1000: just over [`DOWNSAMPLED_ABOVE`], three tiles wide.
const W: u32 = 4200;
const H: u32 = 1000;

fn image_dict(w: u32, h: u32, entries: &[(&str, Object)]) -> Dict {
    let mut d = Dict::new();
    d.set("Type", Object::name("XObject"));
    d.set("Subtype", Object::name("Image"));
    d.set("Width", Object::Int(i64::from(w)));
    d.set("Height", Object::Int(i64::from(h)));
    for (k, v) in entries {
        d.set(*k, v.clone());
    }
    d
}

/// Samples that change at every pixel, so a tile out of place by one shows: `n` bytes a pixel.
fn texture(w: u32, h: u32, n: usize) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).flat_map(move |x| (0..n).map(move |c| (x.wrapping_mul(7) ^ y.wrapping_mul(13) ^ (c as u32 * 85)) as u8))).collect()
}

/// A document with one page `w` × `h` points that draws image `im` (added by `add`) over it
/// after `before` (a fill colour, say), so that at scale 1 each sample is a pixel.
fn page_drawing(w: u32, h: u32, before: &str, add: impl FnOnce(&mut Document) -> ObjRef) -> (Document, ObjRef) {
    let mut doc = Document::new_empty();
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let im = add(&mut doc);
    let mut xobjects = Dict::new();
    xobjects.set("Im", Object::Ref(im));
    let mut resources = Dict::new();
    resources.set("XObject", Object::Dict(xobjects));
    let content = format!("{before} q {w} 0 0 {h} 0 0 cm /Im Do Q");
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    let mut p = Dict::new();
    p.set("Type", Object::name("Page"));
    p.set("Parent", Object::Ref(pages));
    p.set("MediaBox", Object::Array(vec![0.into(), 0.into(), Object::Int(i64::from(w)), Object::Int(i64::from(h))]));
    p.set("Resources", Object::Dict(resources));
    p.set("Contents", Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        d.set("Count", Object::Int(1));
        d.set("Kids", Object::Array(vec![Object::Ref(r)]));
    })
    .unwrap();
    (doc, im)
}

fn render(doc: &Document) -> Vec<u8> {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    out.rgba.to_vec()
}

/// Tiles the image of `doc`, then checks that the image is now a form that draws `count` tiles under
/// the cap, and that the page renders as before, within `tolerance` a channel.
fn tiles_and_looks_the_same(mut doc: Document, im: ObjRef, count: usize, tolerance: u8) {
    let before = render(&doc);
    assert_eq!(tile_large_images(&mut doc), TileReport { tiled: 1, left_whole: 0 });
    let form = doc.get(im);
    let Object::Stream(form) = &*form else { panic!("the image isn't a stream") };
    assert_eq!(form.dict.name(b"Subtype"), Some(b"Form".as_slice()));
    let tiles = form.dict.get(b"Resources").and_then(Object::as_dict).and_then(|r| r.get(b"XObject")).and_then(Object::as_dict).unwrap();
    assert_eq!(tiles.len(), count);
    for (_, tile) in tiles.iter() {
        let tile = doc.get(tile.as_ref().unwrap());
        let Object::Stream(t) = &*tile else { panic!("a tile isn't a stream") };
        let pixels = t.dict.int(b"Width").unwrap() * t.dict.int(b"Height").unwrap();
        assert!(pixels as u64 <= DOWNSAMPLED_ABOVE, "a tile of {pixels} pixels");
    }
    let after = render(&doc);
    assert_eq!(before.len(), after.len());
    let worst = before.iter().zip(&after).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(worst <= tolerance, "the tiled page differs by {worst} in a channel");
}

#[test]
fn rgb_samples_are_cut_to_the_pixel() {
    let (doc, im) = page_drawing(W, H, "", |doc| {
        let d = image_dict(W, H, &[("ColorSpace", Object::name("DeviceRGB")), ("BitsPerComponent", Object::Int(8))]);
        doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 3))))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
}

#[test]
fn one_bit_grey_and_four_bit_indexed_samples_are_cut_on_byte_boundaries() {
    let bits: Vec<u8> = texture(W.div_ceil(8), H, 1);
    let (doc, im) = page_drawing(W, H, "", |doc| {
        let d = image_dict(W, H, &[("ColorSpace", Object::name("DeviceGray")), ("BitsPerComponent", Object::Int(1))]);
        doc.add(Object::Stream(Stream::flate(d, &bits)))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
    let palette: Vec<u8> = (0..48u8).map(|i| i.wrapping_mul(37)).collect();
    let nibbles: Vec<u8> = texture(W.div_ceil(2), H, 1);
    let (doc, im) = page_drawing(W, H, "", |doc| {
        let indexed = Object::Array(vec![
            Object::name("Indexed"),
            Object::name("DeviceRGB"),
            Object::Int(15),
            Object::String(pdfcraft_cos::PdfString::literal(palette.clone())),
        ]);
        let d = image_dict(W, H, &[("ColorSpace", indexed), ("BitsPerComponent", Object::Int(4))]);
        doc.add(Object::Stream(Stream::flate(d, &nibbles)))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
}

#[test]
fn a_cmyk_image_with_a_decode_array_keeps_both() {
    let (doc, im) = page_drawing(W, H, "", |doc| {
        let decode = Object::Array([1, 0, 0, 1, 1, 0, 0, 1].map(Object::Int).to_vec());
        let d = image_dict(W, H, &[("ColorSpace", Object::name("DeviceCMYK")), ("BitsPerComponent", Object::Int(8)), ("Decode", decode)]);
        doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 4))))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
}

#[test]
fn a_soft_mask_is_cut_into_the_same_tiles() {
    let (doc, im) = page_drawing(W, H, "0 0 1 rg 0 0 4200 1000 re f", |doc| {
        // Without `/ColorSpace`, as many writers leave a soft mask.
        let mask = image_dict(W, H, &[("BitsPerComponent", Object::Int(8))]);
        let mask = doc.add(Object::Stream(Stream::flate(mask, &texture(W, H, 1))));
        let d = image_dict(W, H, &[("ColorSpace", Object::name("DeviceRGB")), ("BitsPerComponent", Object::Int(8)), ("SMask", Object::Ref(mask))]);
        doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 3))))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
}

#[test]
fn a_stencil_mask_paints_the_fill_colour_through_its_tiles() {
    let bits: Vec<u8> = texture(W.div_ceil(8), H, 1);
    let (doc, im) = page_drawing(W, H, "1 0 0 rg", |doc| {
        let d = image_dict(W, H, &[("ImageMask", Object::Bool(true))]);
        doc.add(Object::Stream(Stream::flate(d, &bits)))
    });
    tiles_and_looks_the_same(doc, im, 3, 0);
}

#[test]
fn a_jpeg_is_decoded_into_its_tiles() {
    let px = texture(W, H, 3);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode(&px, W, H, image::ExtendedColorType::Rgb8).unwrap();
    let (doc, im) = page_drawing(W, H, "", |doc| {
        let d = image_dict(
            W,
            H,
            &[("ColorSpace", Object::name("DeviceRGB")), ("BitsPerComponent", Object::Int(8)), ("Filter", Object::name("DCTDecode"))],
        );
        doc.add(Object::Stream(Stream::from_raw(d, jpeg)))
    });
    // The renderer's JPEG decoder and the one tiling uses may round a sample differently.
    tiles_and_looks_the_same(doc, im, 3, 2);
}

/// Adds an image to a document.
type AddImage = Box<dyn Fn(&mut Document) -> ObjRef>;

#[test]
fn images_it_cannot_cut_faithfully_are_left_whole() {
    fn rgb() -> Dict {
        image_dict(W, H, &[("ColorSpace", Object::name("DeviceRGB")), ("BitsPerComponent", Object::Int(8))])
    }
    let cases: Vec<(&str, AddImage)> = vec![
        (
            "JPEG 2000",
            Box::new(|doc| {
                let mut d = rgb();
                d.set("Filter", Object::name("JPXDecode"));
                doc.add(Object::Stream(Stream::from_raw(d, b"not decoded here".to_vec())))
            }),
        ),
        ("data short of the image", Box::new(|doc| doc.add(Object::Stream(Stream::flate(rgb(), &texture(W, H - 1, 3)))))),
        (
            "a soft mask of another size",
            Box::new(|doc| {
                let mask = doc.add(Object::Stream(Stream::flate(image_dict(10, 10, &[("BitsPerComponent", Object::Int(8))]), &[0; 100])));
                let mut d = rgb();
                d.set("SMask", Object::Ref(mask));
                doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 3))))
            }),
        ),
        (
            "an unknown colour space",
            Box::new(|doc| {
                let mut d = rgb();
                d.set("ColorSpace", Object::name("Fancy"));
                doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 3))))
            }),
        ),
        (
            "12 bits a component",
            Box::new(|doc| {
                let mut d = rgb();
                d.set("BitsPerComponent", Object::Int(12));
                doc.add(Object::Stream(Stream::flate(d, &texture(W, H, 3))))
            }),
        ),
    ];
    for (what, add) in cases {
        let (mut doc, im) = page_drawing(W, H, "", add);
        let before = doc.get(im);
        assert_eq!(tile_large_images(&mut doc), TileReport { tiled: 0, left_whole: 1 }, "{what}");
        assert!(Arc::ptr_eq(&before, &doc.get(im)) || *before == *doc.get(im), "{what}: changed");
    }
}

#[test]
fn small_images_and_a_large_mask_of_a_small_image_are_not_touched() {
    let (mut doc, im) = page_drawing(W, H, "", |doc| {
        let mask = doc.add(Object::Stream(Stream::flate(image_dict(W, H, &[("BitsPerComponent", Object::Int(8))]), &texture(W, H, 1))));
        // Exactly the cap: drawn whole by Windows.
        let d =
            image_dict(2048, 2048, &[("ColorSpace", Object::name("DeviceGray")), ("BitsPerComponent", Object::Int(8)), ("SMask", Object::Ref(mask))]);
        doc.add(Object::Stream(Stream::flate(d, &texture(2048, 2048, 1))))
    });
    let before = doc.get(im);
    assert_eq!(tile_large_images(&mut doc), TileReport::default());
    assert_eq!(*before, *doc.get(im));
}

#[test]
fn a_grid_of_tiles_keeps_its_rows_and_columns_in_place() {
    // 2100 × 2100: two tiles a side, the top row first in the samples and at the top of the page.
    let (doc, im) = page_drawing(2100, 2100, "", |doc| {
        let d = image_dict(2100, 2100, &[("ColorSpace", Object::name("DeviceRGB")), ("BitsPerComponent", Object::Int(8))]);
        doc.add(Object::Stream(Stream::flate(d, &texture(2100, 2100, 3))))
    });
    tiles_and_looks_the_same(doc, im, 4, 0);
}
