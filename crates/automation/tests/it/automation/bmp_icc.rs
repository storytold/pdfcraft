use super::*;
use pdfcraft_cos::{Document, Object};
use std::sync::Arc;

const BMP_FILE_HEADER: usize = 14;
const BMP_V5_HEADER: usize = 124;
const BMP_PIXEL_OFFSET: usize = BMP_FILE_HEADER + BMP_V5_HEADER;
const PROFILE_DATA_OFFSET: u32 = 132;
const PROFILE_SIZE: usize = 300;
const PROFILE_EMBEDDED: u32 = 0x4D42_4544;

/// A small test-only RGB ICC header, sufficient to verify that the input profile bytes survive.
fn synthetic_rgb_profile() -> Vec<u8> {
    let mut profile = vec![0u8; PROFILE_SIZE];
    profile[..4].copy_from_slice(&(PROFILE_SIZE as u32).to_be_bytes());
    profile[12..16].copy_from_slice(b"prtr");
    profile[16..20].copy_from_slice(b"RGB ");
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    for (index, byte) in profile.iter_mut().enumerate().skip(128) {
        *byte = index as u8;
    }
    profile
}

/// A fixed-header 2×1 BITMAPV5HEADER BMP with two RGB pixels and an embedded profile.
fn bmp_v5_rgb(profile: &[u8]) -> Vec<u8> {
    assert_eq!(profile.len(), PROFILE_SIZE);
    let raster = [70, 80, 90, 10, 20, 30, 0, 0]; // BGR pixels and two row-padding bytes.
    let file_size = BMP_PIXEL_OFFSET + raster.len() + profile.len();
    let mut bmp = vec![0u8; BMP_PIXEL_OFFSET];

    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&(BMP_PIXEL_OFFSET as u32).to_le_bytes());

    let dib = BMP_FILE_HEADER;
    bmp[dib..dib + 4].copy_from_slice(&(BMP_V5_HEADER as u32).to_le_bytes());
    bmp[dib + 4..dib + 8].copy_from_slice(&2i32.to_le_bytes());
    bmp[dib + 8..dib + 12].copy_from_slice(&1i32.to_le_bytes());
    bmp[dib + 12..dib + 14].copy_from_slice(&1u16.to_le_bytes());
    bmp[dib + 14..dib + 16].copy_from_slice(&24u16.to_le_bytes());
    bmp[dib + 16..dib + 20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB.
    bmp[dib + 20..dib + 24].copy_from_slice(&(raster.len() as u32).to_le_bytes());
    bmp[dib + 56..dib + 60].copy_from_slice(&PROFILE_EMBEDDED.to_le_bytes());
    bmp[dib + 112..dib + 116].copy_from_slice(&PROFILE_DATA_OFFSET.to_le_bytes());
    bmp[dib + 116..dib + 120].copy_from_slice(&(PROFILE_SIZE as u32).to_le_bytes());

    bmp.extend_from_slice(&raster);
    bmp.extend_from_slice(profile);
    assert_eq!(bmp.len(), file_size);
    bmp
}

/// Return the first image's ICC profile, component samples and dimensions from a saved PDF.
fn saved_image(pdf: &[u8]) -> (u32, u32, Vec<u8>, i64, Vec<u8>, Vec<u8>) {
    let doc = Document::open(Arc::new(pdf.to_vec())).unwrap();
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let page_tree = doc.get(pages);
    let kids = page_tree.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap();
    let page = doc.resolve(kids.first().unwrap());
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let (width, height, samples, image_dict) = xobjects
        .as_dict()
        .unwrap()
        .iter()
        .find_map(|(_, value)| {
            let object = doc.resolve(value);
            match &*object {
                Object::Stream(stream) if stream.dict.name(b"Subtype") == Some(&b"Image"[..]) => Some((
                    stream.dict.int(b"Width").unwrap() as u32,
                    stream.dict.int(b"Height").unwrap() as u32,
                    stream.decoded().unwrap(),
                    stream.dict.clone(),
                )),
                _ => None,
            }
        })
        .expect("saved page has an image XObject");
    let color_space = image_dict.get(b"ColorSpace").unwrap().as_array().unwrap();
    assert_eq!(color_space.first().and_then(Object::as_name), Some(&b"ICCBased"[..]));
    let profile = doc.resolve(color_space.get(1).unwrap());
    let Object::Stream(profile) = &*profile else { panic!("ICCBased profile stream") };
    (width, height, samples, profile.dict.int(b"N").unwrap(), profile.dict.name(b"Alternate").unwrap().to_vec(), profile.decoded().unwrap())
}

fn rendered_page(a: &mut Automation, doc: u64) -> (u32, u32) {
    let content = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { width, height, .. } = content.first().unwrap() else { panic!("rendered page PNG") };
    (*width, *height)
}

#[test]
fn bmp_v5_icc_survives_create_add_save_reopen_and_preserves_source() {
    let profile = synthetic_rgb_profile();
    let source = bmp_v5_rgb(&profile);
    let dir = workdir("bmp-v5-icc");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/profiled.bmp"), &source).unwrap();
    let expected_samples = [90, 80, 70, 30, 20, 10];
    let mut a = auto(&dir);

    let created = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/profiled.bmp"] }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_save", json!({ "doc": created, "path": "created.pdf" }));
    ok(&mut a, "doc_close", json!({ "doc": created }));
    let created_reopened = ok(&mut a, "doc_open", json!({ "path": "created.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(rendered_page(&mut a, created_reopened), (2, 1), "Create output renders after save and reopen");
    assert_eq!(
        saved_image(&std::fs::read(dir.join("created.pdf")).unwrap()),
        (2, 1, expected_samples.to_vec(), 3, b"DeviceRGB".to_vec(), profile.clone())
    );
    ok(&mut a, "doc_close", json!({ "doc": created_reopened }));

    let blank = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 64, "height": 32 }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_image", json!({ "doc": blank, "page": 1, "path": "src/profiled.bmp", "rect": [0, 0, 2, 1] }));
    ok(&mut a, "doc_save", json!({ "doc": blank, "path": "added.pdf" }));
    ok(&mut a, "doc_close", json!({ "doc": blank }));
    let added_reopened = ok(&mut a, "doc_open", json!({ "path": "added.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(rendered_page(&mut a, added_reopened), (64, 32), "Add Image output renders after save and reopen");
    assert_eq!(saved_image(&std::fs::read(dir.join("added.pdf")).unwrap()), (2, 1, expected_samples.to_vec(), 3, b"DeviceRGB".to_vec(), profile));

    assert_eq!(std::fs::read(dir.join("src/profiled.bmp")).unwrap(), source, "both import workflows leave the source unchanged");
    let _ = std::fs::remove_dir_all(dir);
}
