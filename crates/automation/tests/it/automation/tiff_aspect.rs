use super::*;
use pdfcraft_cos::{Document, Object};
use std::path::Path;
use std::sync::Arc;

const TIFF_PIXELS: [u8; 18] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255];

/// A contributor-generated 3×2 chunky RGB TIFF with unitless X/Y resolution rationals.
fn unitless_rgb_tiff(x: (u32, u32), y: (u32, u32)) -> Vec<u8> {
    // Classic little-endian TIFF with 13 sorted IFD entries.
    const ENTRY_COUNT: u16 = 13;
    const BITS_OFFSET: u32 = 170;
    const X_RESOLUTION_OFFSET: u32 = 176;
    const Y_RESOLUTION_OFFSET: u32 = 184;
    const PIXEL_OFFSET: u32 = 192;

    let mut bytes = b"II*\0".to_vec();
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&ENTRY_COUNT.to_le_bytes());
    let inline_short = |value: u16| [value as u8, (value >> 8) as u8, 0, 0];

    {
        let mut entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&value);
        };
        entry(256, 4, 1, 3u32.to_le_bytes()); // ImageWidth.
        entry(257, 4, 1, 2u32.to_le_bytes()); // ImageLength.
        entry(258, 3, 3, BITS_OFFSET.to_le_bytes()); // BitsPerSample.
        entry(259, 3, 1, inline_short(1)); // Compression = none.
        entry(262, 3, 1, inline_short(2)); // PhotometricInterpretation = RGB.
        entry(273, 4, 1, PIXEL_OFFSET.to_le_bytes()); // StripOffsets.
        entry(277, 3, 1, inline_short(3)); // SamplesPerPixel.
        entry(278, 4, 1, 2u32.to_le_bytes()); // RowsPerStrip.
        entry(279, 4, 1, (TIFF_PIXELS.len() as u32).to_le_bytes()); // StripByteCounts.
        entry(282, 5, 1, X_RESOLUTION_OFFSET.to_le_bytes());
        entry(283, 5, 1, Y_RESOLUTION_OFFSET.to_le_bytes());
        entry(284, 3, 1, inline_short(1)); // PlanarConfiguration = chunky.
        entry(296, 3, 1, inline_short(1)); // ResolutionUnit = none.
    }
    bytes.extend_from_slice(&0u32.to_le_bytes()); // No next IFD.

    assert_eq!(bytes.len(), BITS_OFFSET as usize);
    for _ in 0..3 {
        bytes.extend_from_slice(&8u16.to_le_bytes());
    }
    bytes.extend_from_slice(&x.0.to_le_bytes());
    bytes.extend_from_slice(&x.1.to_le_bytes());
    bytes.extend_from_slice(&y.0.to_le_bytes());
    bytes.extend_from_slice(&y.1.to_le_bytes());
    assert_eq!(bytes.len(), PIXEL_OFFSET as usize);
    bytes.extend_from_slice(&TIFF_PIXELS);
    bytes
}

fn saved_rgb_image(path: &Path) -> (u32, u32, Vec<u8>) {
    let doc = Document::open(Arc::new(std::fs::read(path).unwrap())).unwrap();
    let root = doc.get(doc.root().unwrap());
    let pages = doc.get(root.as_dict().unwrap().reference(b"Pages").unwrap());
    let kids = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap();
    let page = doc.resolve(&kids[0]);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let (width, height, samples) = xobjects
        .as_dict()
        .unwrap()
        .iter()
        .find_map(|(_, value)| {
            let object = doc.resolve(value);
            match &*object {
                Object::Stream(stream) if stream.dict.name(b"Subtype") == Some(&b"Image"[..]) => {
                    Some((stream.dict.int(b"Width").unwrap() as u32, stream.dict.int(b"Height").unwrap() as u32, stream.decoded().unwrap()))
                }
                _ => None,
            }
        })
        .expect("saved PDF has an image XObject");
    (width, height, samples)
}

#[test]
fn unitless_tiff_aspect_survives_create_add_save_reopen_and_explicit_dpi() {
    let dir = workdir("unitless-tiff-aspect");
    let mut a = auto(&dir);

    for (name, x, y) in [("half-quarter.tiff", (1, 2), (1, 4)), ("two-one.tiff", (2, 1), (1, 1))] {
        let source = unitless_rgb_tiff(x, y);
        std::fs::write(dir.join(name), &source).unwrap();

        let created = ok(&mut a, "doc_create", json!({"from": "images", "paths": [name]}))["doc"].as_u64().unwrap();
        let info = ok(&mut a, "doc_info", json!({"doc": created}));
        assert_eq!(
            (info["pages"][0]["width"].as_f64(), info["pages"][0]["height"].as_f64()),
            (Some(3.0), Some(4.0)),
            "unitless TIFF ratios {x:?}/{y:?} normalize to the same 3×4 pt page"
        );

        let pdf_name = format!("{name}.pdf");
        ok(&mut a, "doc_save", json!({"doc": created, "path": pdf_name}));
        ok(&mut a, "doc_close", json!({"doc": created}));
        let reopened = ok(&mut a, "doc_open", json!({"path": pdf_name}))["doc"].as_u64().unwrap();
        let rendered = a.call("page_render", &json!({"doc": reopened, "page": 1, "dpi": 144})).unwrap();
        let Content::Png { width, height, .. } = &rendered[0] else { panic!("rendered TIFF page") };
        assert_eq!((*width, *height), (6, 8));
        assert_eq!(saved_rgb_image(&dir.join(&pdf_name)), (3, 2, TIFF_PIXELS.to_vec()));
        ok(&mut a, "doc_close", json!({"doc": reopened}));

        let blank = ok(&mut a, "doc_create", json!({"from": "blank", "width": 64, "height": 32}))["doc"].as_u64().unwrap();
        let added = ok(&mut a, "page_add_image", json!({"doc": blank, "page": 1, "path": name}));
        let rect: Vec<f64> = added["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!((rect[2] - rect[0], rect[3] - rect[1]), (3.0, 4.0));
        let added_pdf = format!("added-{name}.pdf");
        ok(&mut a, "doc_save", json!({"doc": blank, "path": added_pdf}));
        ok(&mut a, "doc_close", json!({"doc": blank}));
        let added_reopened = ok(&mut a, "doc_open", json!({"path": added_pdf}))["doc"].as_u64().unwrap();
        let rendered = a.call("page_render", &json!({"doc": added_reopened, "page": 1, "dpi": 72})).unwrap();
        let Content::Png { width, height, .. } = &rendered[0] else { panic!("rendered Add image page") };
        assert_eq!((*width, *height), (64, 32));
        assert_eq!(saved_rgb_image(&dir.join(&added_pdf)), (3, 2, TIFF_PIXELS.to_vec()));
        ok(&mut a, "doc_close", json!({"doc": added_reopened}));

        let explicit = ok(&mut a, "doc_create", json!({"from": "images", "paths": [name], "dpi": 144}))["doc"].as_u64().unwrap();
        let info = ok(&mut a, "doc_info", json!({"doc": explicit}));
        assert_eq!(
            (info["pages"][0]["width"].as_f64(), info["pages"][0]["height"].as_f64()),
            (Some(1.5), Some(1.0)),
            "explicit DPI overrides unitless aspect metadata"
        );
        assert_eq!(std::fs::read(dir.join(name)).unwrap(), source, "Create and Add image leave the TIFF bytes unchanged");
    }
}
