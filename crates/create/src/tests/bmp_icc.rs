use super::*;
use image::ImageEncoder;

const LCS_SRGB: u32 = 0x7352_4742;
const PROFILE_LINKED: u32 = 0x4C49_4E4B;
const PROFILE_EMBEDDED: u32 = 0x4D42_4544;
const MAX_BMP_ICC_BYTES: usize = 16 << 20;

fn bmp_bytes(width: u32, height: u32, color: image::ExtendedColorType, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bytes).write_image(pixels, width, height, color).unwrap();
    bytes
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes.get(at..at + 4).unwrap().try_into().unwrap())
}

/// Upgrade a source-generated BMP to BITMAPV5HEADER and optionally append the profile payload.
/// `profile_data` is relative to the start of the DIB header, and the profile follows the pixels.
fn bmp_v5(base: &[u8], color_space: u32, profile_payload: &[u8], profile_data: Option<u32>, profile_size: Option<u32>) -> Vec<u8> {
    assert!(base.starts_with(b"BM"));
    let old_offset = usize::try_from(le_u32(base, 10)).unwrap();
    let old_header_size = usize::try_from(le_u32(base, 14)).unwrap();
    assert!(old_header_size <= 124);
    let old_header_end = 14 + old_header_size;
    let table = base.get(old_header_end..old_offset).unwrap();
    let pixels = base.get(old_offset..).unwrap();

    let mut header = base.get(14..old_header_end).unwrap().to_vec();
    header.resize(124, 0);
    header[..4].copy_from_slice(&124u32.to_le_bytes());
    header[56..60].copy_from_slice(&color_space.to_le_bytes());
    let default_profile_data = u32::try_from(124 + table.len() + pixels.len()).unwrap();
    let profile_data = profile_data.unwrap_or(if profile_payload.is_empty() { 0 } else { default_profile_data });
    header[112..116].copy_from_slice(&profile_data.to_le_bytes());
    header[116..120].copy_from_slice(&profile_size.unwrap_or(profile_payload.len() as u32).to_le_bytes());

    let new_pixel_offset = 14 + header.len() + table.len();
    let mut out = base.get(..14).unwrap().to_vec();
    out.extend_from_slice(&header);
    out.extend_from_slice(table);
    out.extend_from_slice(pixels);
    out.extend_from_slice(profile_payload);
    let file_size = u32::try_from(out.len()).unwrap();
    out[2..6].copy_from_slice(&file_size.to_le_bytes());
    out[10..14].copy_from_slice(&u32::try_from(new_pixel_offset).unwrap().to_le_bytes());
    out
}

type ImageParts = (Dict, Vec<u8>, Option<(Dict, Vec<u8>)>);

fn image_parts(doc: &Document) -> ImageParts {
    let page = pages(doc).remove(0);
    let resources = doc.resolve(page.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let image = doc.resolve(xobjects.as_dict().unwrap().get(b"Im0").unwrap());
    let Object::Stream(image) = &*image else { panic!("image XObject") };
    let mask = image.dict.get(b"SMask").map(|reference| {
        let mask = doc.resolve(reference);
        let Object::Stream(mask) = &*mask else { panic!("soft mask XObject") };
        (mask.dict.clone(), mask.decoded().unwrap())
    });
    (image.dict.clone(), image.decoded().unwrap(), mask)
}

#[test]
fn bmp_v5_embedded_rgb_profile_survives_create_save_reopen_and_image_xobject() {
    let pixels = [128, 128, 128, 128, 64, 32];
    let profile = icc_profile(b"RGB ");
    let base = bmp_bytes(2, 1, image::ExtendedColorType::Rgb8, &pixels);
    let bytes = bmp_v5(&base, PROFILE_EMBEDDED, &profile, None, None);

    let embedded = embed("profiled.bmp", &bytes, 1).unwrap().remove(0);
    assert_eq!(embedded.icc, Some(profile.clone()));
    assert_eq!(embedded.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    assert_eq!(embedded.data, pixels);
    assert_eq!(embedded.px, (2, 1));
    assert_eq!(embedded.dpi, (72.0, 72.0), "BMP resolution remains at the existing fallback");

    let doc = reopen(&from_images(&[("profiled.bmp".into(), bytes.clone())]).unwrap());
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 2.0, 1.0]);
    let (image, samples, mask) = image_parts(&doc);
    assert!(mask.is_none());
    assert_eq!(samples, pixels);
    let (n, alternate, actual, _) = icc_of(&doc, &image).expect("BMP V5 profile survives save/reopen");
    assert_eq!((n, alternate.as_slice(), actual), (3, &b"DeviceRGB"[..], profile.clone()));

    let mut doc = Document::new_empty();
    let (reference, size) = image_xobject(&mut doc, "profiled.bmp", &bytes).unwrap();
    assert_eq!(size, (2.0, 1.0));
    let Object::Stream(image) = &*doc.get(reference) else { panic!("image XObject") };
    let (n, alternate, actual, _) = icc_of(&doc, &image.dict).expect("image_xobject keeps the BMP profile");
    assert_eq!((n, alternate.as_slice(), actual), (3, &b"DeviceRGB"[..], profile));
    assert_eq!(image.decoded().unwrap(), pixels);
}

#[test]
fn bmp_v5_bitfields_alpha_keeps_rgb_profile_on_color_and_soft_mask_separate() {
    let rgba = [70, 70, 70, 128, 90, 80, 70, 255];
    let profile = icc_profile(b"RGB ");
    let base = bmp_bytes(2, 1, image::ExtendedColorType::Rgba8, &rgba);
    assert_eq!(le_u32(&base, 14), 108, "the source encoder writes a V4 BITFIELDS header");
    let bytes = bmp_v5(&base, PROFILE_EMBEDDED, &profile, None, None);

    let doc = reopen(&from_images(&[("alpha.bmp".into(), bytes)]).unwrap());
    let (image, samples, mask) = image_parts(&doc);
    assert_eq!(samples, [70, 70, 70, 90, 80, 70]);
    let (n, alternate, actual, _) = icc_of(&doc, &image).unwrap();
    assert_eq!((n, alternate.as_slice(), actual), (3, &b"DeviceRGB"[..], profile));
    let (mask, alpha) = mask.expect("BMP alpha is retained as a soft mask");
    assert_eq!(mask.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
    assert_eq!(alpha, [128, 255]);
    assert!(!mask.contains(b"ICCBased"));
}

#[test]
fn bmp_v5_indexed_neutral_palette_retains_rgb_profile_and_rgb_samples() {
    let palette_indices = [32, 128, 224];
    let profile = icc_profile(b"RGB ");
    let base = bmp_bytes(3, 1, image::ExtendedColorType::L8, &palette_indices);
    let bytes = bmp_v5(&base, PROFILE_EMBEDDED, &profile, None, None);

    let doc = reopen(&from_images(&[("indexed.bmp".into(), bytes)]).unwrap());
    let (image, samples, mask) = image_parts(&doc);
    assert!(mask.is_none());
    assert_eq!(samples, [32, 32, 32, 128, 128, 128, 224, 224, 224]);
    let (n, alternate, actual, _) = icc_of(&doc, &image).expect("indexed BMP's RGB profile is retained");
    assert_eq!((n, alternate.as_slice(), actual), (3, &b"DeviceRGB"[..], profile));
}

#[test]
fn bmp_v4_and_bmp_v5_without_embedded_profile_keep_device_color() {
    let pixels = [90, 80, 70, 30, 20, 10];
    let base = bmp_bytes(2, 1, image::ExtendedColorType::Rgb8, &pixels);
    let v4_rgba = bmp_bytes(2, 1, image::ExtendedColorType::Rgba8, &[90, 80, 70, 255, 30, 20, 10, 255]);
    assert_eq!(le_u32(&v4_rgba, 14), 108);
    let without_tag = bmp_v5(&base, LCS_SRGB, &[], None, Some(0));

    for bytes in [v4_rgba, without_tag] {
        let embedded = embed("untagged.bmp", &bytes, 1).unwrap().remove(0);
        assert!(embedded.icc.is_none());
        assert_eq!(embedded.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
        assert_eq!(embedded.data.len(), 6);
    }
}

#[test]
fn bmp_v5_bad_or_unsupported_profiles_fall_back_without_losing_pixels() {
    let pixels = [90, 80, 70, 30, 20, 10];
    let base = bmp_bytes(2, 1, image::ExtendedColorType::Rgb8, &pixels);
    let good = icc_profile(b"RGB ");
    let mut bad_signature = good.clone();
    bad_signature[36] = b'x';

    let linked_path = b"profile.icc\0";
    let truncated_profile = &good[..64];
    let oversized = (MAX_BMP_ICC_BYTES + 1) as u32;
    let cases = [
        ("missing", bmp_v5(&base, LCS_SRGB, &[], None, Some(0))),
        ("linked profile", bmp_v5(&base, PROFILE_LINKED, linked_path, None, None)),
        ("wrong component space", bmp_v5(&base, PROFILE_EMBEDDED, &icc_profile(b"GRAY"), None, None)),
        ("short header", bmp_v5(&base, PROFILE_EMBEDDED, &[0; 127], None, None)),
        ("bad signature", bmp_v5(&base, PROFILE_EMBEDDED, &bad_signature, None, None)),
        ("zero profile size", bmp_v5(&base, PROFILE_EMBEDDED, &good, None, Some(0))),
        ("zero profile offset", bmp_v5(&base, PROFILE_EMBEDDED, &good, Some(0), None)),
        ("out-of-range profile offset", bmp_v5(&base, PROFILE_EMBEDDED, &good, Some(u32::MAX - 8), None)),
        ("truncated profile data", bmp_v5(&base, PROFILE_EMBEDDED, truncated_profile, None, Some(good.len() as u32))),
        ("over-limit declared profile size", bmp_v5(&base, PROFILE_EMBEDDED, &good, None, Some(oversized))),
    ];

    for (label, bytes) in cases {
        let embedded = embed("fallback.bmp", &bytes, 1).unwrap().remove(0);
        assert!(embedded.icc.is_none(), "{label}");
        assert_eq!(embedded.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]), "{label}");
        assert_eq!(embedded.data, pixels, "{label}");
        let doc = reopen(&from_images(&[("fallback.bmp".into(), bytes)]).unwrap());
        assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]), "{label}");
    }
}

#[test]
fn bmp_v5_profile_range_inside_pixel_payload_is_ignored() {
    let profile = icc_profile(b"RGB ");
    let disk_pixels = profile.as_chunks::<3>().0.iter().flat_map(|bgr| [bgr[2], bgr[1], bgr[0]]).collect::<Vec<_>>();
    let base = bmp_bytes(100, 1, image::ExtendedColorType::Rgb8, &disk_pixels);
    assert_eq!(le_u32(&base, 14), 40);

    // The pixel bytes themselves form a plausible ICC profile, but are not profile storage.
    let overlapped = bmp_v5(&base, PROFILE_EMBEDDED, &[], Some(124), Some(profile.len() as u32));
    let control = bmp_v5(&base, LCS_SRGB, &[], Some(0), Some(0));

    let overlapped = embed("overlap.bmp", &overlapped, 1).unwrap().remove(0);
    let control = embed("control.bmp", &control, 1).unwrap().remove(0);
    assert!(overlapped.icc.is_none());
    assert_eq!(overlapped.dict.name(b"ColorSpace"), control.dict.name(b"ColorSpace"));
    assert_eq!(overlapped.data, control.data);
    assert_eq!(overlapped.data, disk_pixels);
}

#[test]
fn bmp_v5_profile_range_inside_unused_palette_entries_is_ignored() {
    let indices = [200, 201, 202];
    let base = bmp_bytes(3, 1, image::ExtendedColorType::L8, &indices);
    let pixel_offset = usize::try_from(le_u32(&base, 10)).unwrap();
    let palette_start = 14 + usize::try_from(le_u32(&base, 14)).unwrap();
    assert!(pixel_offset - palette_start >= 300, "the indexed BMP has a full palette");

    let profile = icc_profile(b"RGB ");
    let mut palette_overlap = base.clone();
    palette_overlap[palette_start..palette_start + profile.len()].copy_from_slice(&profile);
    let overlapped = bmp_v5(&palette_overlap, PROFILE_EMBEDDED, &[], Some(124), Some(profile.len() as u32));
    let control = bmp_v5(&palette_overlap, LCS_SRGB, &[], Some(0), Some(0));

    let overlapped = embed("palette-overlap.bmp", &overlapped, 1).unwrap().remove(0);
    let control = embed("palette-control.bmp", &control, 1).unwrap().remove(0);
    assert!(overlapped.icc.is_none());
    assert_eq!(overlapped.dict.name(b"ColorSpace"), control.dict.name(b"ColorSpace"));
    assert_eq!(overlapped.data, control.data);
}

#[test]
fn bmp_v5_rle8_embedded_profile_after_compressed_pixels_is_retained() {
    let indices = [32, 128, 224];
    let base = bmp_bytes(3, 1, image::ExtendedColorType::L8, &indices);
    let mut bytes = bmp_v5(&base, PROFILE_EMBEDDED, &icc_profile(b"RGB "), None, None);
    let pixel_offset = usize::try_from(le_u32(&bytes, 10)).unwrap();
    let profile_offset = 14 + usize::try_from(le_u32(&bytes, 126)).unwrap();
    let compressed = [0, 3, 32, 128, 224, 0, 0, 1];
    let old_pixel_end = profile_offset;
    assert!(pixel_offset <= old_pixel_end);
    bytes.splice(pixel_offset..old_pixel_end, compressed);

    bytes[30..34].copy_from_slice(&1u32.to_le_bytes()); // BI_RLE8
    bytes[34..38].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
    let table_len = pixel_offset - (14 + 124);
    let new_profile_data = 124 + table_len + compressed.len();
    bytes[126..130].copy_from_slice(&(new_profile_data as u32).to_le_bytes());
    let file_size = u32::try_from(bytes.len()).unwrap();
    bytes[2..6].copy_from_slice(&file_size.to_le_bytes());

    let embedded = embed("rle8.bmp", &bytes, 1).unwrap().remove(0);
    assert_eq!(embedded.icc, Some(icc_profile(b"RGB ")));
    assert_eq!(embedded.data, [32, 32, 32, 128, 128, 128, 224, 224, 224]);
}

#[test]
fn bmp_v5_fully_present_profile_above_byte_cap_is_ignored() {
    let pixels = [90, 80, 70, 30, 20, 10];
    let base = bmp_bytes(2, 1, image::ExtendedColorType::Rgb8, &pixels);
    let mut profile = icc_profile(b"RGB ");
    profile.resize(MAX_BMP_ICC_BYTES + 1, 0);
    let size = u32::try_from(profile.len()).unwrap();
    profile[..4].copy_from_slice(&size.to_be_bytes());
    let bytes = bmp_v5(&base, PROFILE_EMBEDDED, &profile, None, None);
    let embedded = embed("over-cap.bmp", &bytes, 1).unwrap().remove(0);
    assert!(embedded.icc.is_none(), "a complete profile must still obey the metadata cap");
    assert_eq!(embedded.data, pixels);
    assert_eq!(embedded.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
}

#[test]
fn bmp_v5_rejects_profiles_with_mismatched_declared_size() {
    let pixels = [90, 80, 70];
    let base = bmp_bytes(1, 1, image::ExtendedColorType::Rgb8, &pixels);
    let profile = icc_profile(b"RGB ");
    for declared_size in [128usize, profile.len() - 1, profile.len() + 1, profile.len()] {
        let mut candidate = profile.clone();
        candidate[..4].copy_from_slice(&(declared_size as u32).to_be_bytes());
        let bytes = bmp_v5(&base, PROFILE_EMBEDDED, &candidate, None, None);
        let image = embed("size-mismatch.bmp", &bytes, 1).unwrap().remove(0);

        if declared_size == profile.len() {
            assert_eq!(image.icc, Some(profile.clone()), "a matching declared size retains the profile");
        } else {
            assert!(image.icc.is_none(), "declared size {declared_size} must be rejected");
            assert_eq!(image.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
        }
        assert_eq!(image.data, pixels, "profile validation must not alter source samples");
    }
}
