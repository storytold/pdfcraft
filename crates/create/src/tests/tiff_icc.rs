use super::*;

/// Contributor-original uncompressed TIFF, with an optional UNDEFINED ICC field.
fn tagged_tiff(photo: u16, channels: u16, bits: u16, pixels: &[u8], profile: Option<&[u8]>) -> Vec<u8> {
    let width = pixels.len() * 8 / usize::from(channels) / usize::from(bits);
    let short = |value: u16| value.to_le_bytes().to_vec();
    let long = |value: u32| value.to_le_bytes().to_vec();
    let mut tags = vec![
        (256u16, 4u16, 1u32, long(width as u32)),
        (257, 4, 1, long(1)),
        (258, 3, u32::from(channels), short(bits).repeat(usize::from(channels))),
        (259, 3, 1, short(1)),
        (262, 3, 1, short(photo)),
        (273, 4, 1, long(0)),
        (277, 3, 1, short(channels)),
        (278, 4, 1, long(1)),
        (279, 4, 1, long(pixels.len() as u32)),
    ];
    if (photo == 1 && channels == 2) || (photo == 2 && channels == 4) {
        tags.push((338, 3, 1, short(2))); // unassociated alpha
    }
    if let Some(profile) = profile {
        tags.push((34675, 7, profile.len() as u32, profile.to_vec()));
    }
    tags.sort_by_key(|tag| tag.0);
    let extra_start = 8 + 2 + tags.len() * 12 + 4;
    let mut extra = Vec::new();
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    let strip_entry = out.len() + tags.iter().position(|tag| tag.0 == 273).unwrap() * 12 + 8;
    for (tag, kind, count, mut data) in tags {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if data.len() <= 4 {
            data.resize(4, 0);
            out.extend_from_slice(&data);
        } else {
            out.extend_from_slice(&((extra_start + extra.len()) as u32).to_le_bytes());
            extra.extend_from_slice(&data);
            if extra.len() % 2 != 0 {
                extra.push(0);
            }
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&extra);
    let offset = out.len() as u32;
    out[strip_entry..strip_entry + 4].copy_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(pixels);
    out
}

#[test]
fn tiff_icc_keeps_supported_color_samples_and_profiles() {
    for (photo, channels, space, device) in [(1, 1, b"GRAY", "DeviceGray"), (2, 3, b"RGB ", "DeviceRGB"), (5, 4, b"CMYK", "DeviceCMYK")] {
        let profile = icc_profile(space);
        for bits in [8, 16] {
            let samples = if bits == 8 { vec![90u8; channels as usize] } else { [7, 90].repeat(channels as usize) };
            let bytes = tagged_tiff(photo, channels, bits, &samples, Some(&profile));
            let doc = reopen(&from_images(&[("color.tiff".into(), bytes.clone())]).unwrap());
            let (n, alternate, actual, _) = icc_of(&doc, &image_of(&doc, 0)).expect("TIFF profile survives save/reopen");
            assert_eq!((n, alternate, actual), (i64::from(channels), device.as_bytes().to_vec(), profile.clone()));
            let image = embed("color.tiff", &bytes, 1).unwrap().remove(0);
            assert_eq!(image.data, vec![90; channels as usize]);
            let mut doc = Document::new_empty();
            let (r, _) = image_xobject(&mut doc, "color.tiff", &bytes).unwrap();
            let Object::Stream(s) = &*doc.get(r) else { panic!() };
            assert_eq!(icc_of(&doc, &s.dict).unwrap().2, profile);
        }
    }
    let profile = icc_profile(b"GRAY");
    let bytes = tagged_tiff(1, 1, 1, &[0b1010_0110], Some(&profile));
    let image = embed("scan.tiff", &bytes, 1).unwrap().remove(0);
    assert_eq!(image.data, [0b1010_0110]);
    assert_eq!(image.icc, Some(profile.clone()));
    let doc = reopen(&from_images(&[("scan.tiff".into(), bytes)]).unwrap());
    assert_eq!(icc_of(&doc, &image_of(&doc, 0)).map(|(n, _, p, _)| (n, p)), Some((1, profile)));

    // BigTIFF uses the same bounded raw profile read, including profiles above 1 MiB.
    let mut profile = icc_profile(b"RGB ");
    profile.resize((1 << 20) + 1, 0);
    let profile_len = profile.len() as u32;
    profile[..4].copy_from_slice(&profile_len.to_be_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new_big(&mut out).unwrap();
        let mut page = encoder.new_image::<tiff::encoder::colortype::RGB8>(1, 1).unwrap();
        page.encoder().write_tag(tiff::tags::Tag::IccProfile, profile.as_slice()).unwrap();
        page.write_data(&[90, 80, 70]).unwrap();
    }
    let bytes = out.into_inner();
    assert!(bytes.starts_with(b"II+\0"));
    assert_eq!(embed("big.tiff", &bytes, 1).unwrap().remove(0).icc, Some(profile));
}

#[test]
fn tiff_icc_alpha_keeps_the_profile_component_space() {
    for (photo, channels, space, color) in [(2, 4, b"RGB ", vec![90, 90, 90]), (2, 4, b"RGB ", vec![90, 80, 70])] {
        let profile = icc_profile(space);
        for bits in [8, 16] {
            let mut samples = color.clone();
            samples.push(128);
            if bits == 16 {
                samples = samples.into_iter().flat_map(|v| [0, v]).collect();
            }
            let bytes = tagged_tiff(photo, channels, bits, &samples, Some(&profile));
            let image = embed("alpha.tiff", &bytes, 1).unwrap().remove(0);
            assert_eq!(image.icc, Some(profile.clone()));
            assert_eq!(image.data, color, "profiled RGB remains RGB even with neutral channel values");
            let (mask, alpha) = image.smask.unwrap();
            assert_eq!(mask.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
            assert_eq!(alpha, [128]);
            assert!(!mask.contains(b"ICCBased"));
            let doc = reopen(&from_images(&[("alpha.tiff".into(), bytes)]).unwrap());
            let dict = image_of(&doc, 0);
            assert_eq!(icc_of(&doc, &dict).map(|(n, _, p, _)| (n, p)), Some((i64::from(channels - 1), profile.clone())));
            let mask = doc.resolve(dict.get(b"SMask").unwrap());
            let Object::Stream(mask) = &*mask else { panic!() };
            assert_eq!(mask.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
            assert_eq!(mask.decoded().unwrap(), [128]);
        }
    }
}

#[test]
fn tiff_icc_is_per_directory_and_shares_identical_profiles() {
    let profile = icc_profile(b"RGB ");
    let mut different = profile.clone();
    different[128] ^= 1;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new(&mut out).unwrap();
        for profile in [Some(&profile), Some(&different), None, Some(&profile)] {
            let mut page = encoder.new_image::<tiff::encoder::colortype::RGB8>(1, 1).unwrap();
            if let Some(profile) = profile {
                page.encoder().write_tag(tiff::tags::Tag::IccProfile, profile.as_slice()).unwrap();
            }
            page.write_data(&[90, 80, 70]).unwrap();
        }
    }
    let bytes = out.into_inner();
    let doc = reopen(&from_images(&[("pages.tiff".into(), bytes)]).unwrap());
    let first = icc_of(&doc, &image_of(&doc, 0)).unwrap();
    assert_eq!(first.2, profile);
    assert_eq!(icc_of(&doc, &image_of(&doc, 1)).unwrap().2, different);
    assert_eq!(image_of(&doc, 2).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    assert_eq!(first.3, icc_of(&doc, &image_of(&doc, 3)).unwrap().3);
}

#[test]
fn tiff_icc_counts_toward_the_retained_budget() {
    let profile = icc_profile(b"RGB ");
    let bytes = tagged_tiff(2, 3, 8, &[90, 80, 70], Some(&profile));
    assert!(tiff_pages_within("budget.tiff", &bytes, 1, profile.len() + 2).is_err());
    assert!(tiff_pages_within("budget.tiff", &bytes, 1, profile.len() + 3).is_ok());
}

#[test]
fn tiff_icc_impossible_count_returns_before_reading_profile_bytes() {
    let profile = icc_profile(b"RGB ");
    let mut bytes = tagged_tiff(2, 3, 8, &[90, 80, 70], Some(&profile));
    let impossible_count = bytes.len() as u32 + 1;
    assert!(impossible_count < MAX_TIFF_ICC_BYTES as u32);
    bytes[122..126].copy_from_slice(&impossible_count.to_le_bytes());
    let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    // Start away from the profile: even a failed exact read may otherwise leave the cursor there.
    decoder.inner().set_position(0);
    let before = decoder.inner().position();
    assert!(tiff_icc(&mut decoder, 3).is_none());
    assert_eq!(decoder.inner().position(), before, "an impossible count must return before attempting profile I/O");
}

#[test]
fn tiff_icc_missing_malformed_and_mismatched_profiles_keep_device_color() {
    let mut broken = icc_profile(b"RGB ");
    broken[36] = 0;
    for profile in [None, Some(Vec::new()), Some(vec![0; 127]), Some(broken), Some(icc_profile(b"GRAY"))] {
        let bytes = tagged_tiff(2, 3, 8, &[90, 80, 70], profile.as_deref());
        let image = embed("fallback.tiff", &bytes, 1).unwrap().remove(0);
        assert!(image.icc.is_none());
        assert_eq!(image.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
        assert_eq!(image.data, [90, 80, 70]);
        let doc = reopen(&from_images(&[("fallback.tiff".into(), bytes)]).unwrap());
        assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    }
    let profile = icc_profile(b"RGB ");
    let mut oversized = tagged_tiff(2, 3, 8, &[90, 80, 70], Some(&profile));
    // The last entry's count claims more than the app's metadata ceiling, without a large fixture.
    oversized[122..126].copy_from_slice(&((MAX_TIFF_ICC_BYTES + 1) as u32).to_le_bytes());
    let image = embed("oversized.tiff", &oversized, 1).unwrap().remove(0);
    assert!(image.icc.is_none());
    assert_eq!(image.data, [90, 80, 70]);
    for (offset, data) in [(120, 3u16.to_le_bytes().to_vec()), (126, ((oversized.len() - 10) as u32).to_le_bytes().to_vec())] {
        let mut malformed = tagged_tiff(2, 3, 8, &[90, 80, 70], Some(&profile));
        malformed[offset..offset + data.len()].copy_from_slice(&data);
        let image = embed("malformed.tiff", &malformed, 1).unwrap().remove(0);
        assert!(image.icc.is_none(), "non-byte or truncated profile is ignored");
        assert_eq!(image.data, [90, 80, 70]);
    }
}

#[test]
fn tiff_icc_rejects_profiles_with_mismatched_declared_size() {
    let profile = icc_profile(b"RGB ");
    for declared_size in [128usize, profile.len() - 1, profile.len() + 1, profile.len()] {
        let mut candidate = profile.clone();
        candidate[..4].copy_from_slice(&(declared_size as u32).to_be_bytes());
        let bytes = tagged_tiff(2, 3, 8, &[90, 80, 70], Some(&candidate));
        let image = embed("size-mismatch.tiff", &bytes, 1).unwrap().remove(0);

        if declared_size == profile.len() {
            assert_eq!(image.icc, Some(profile.clone()), "a matching declared size retains the profile");
        } else {
            assert!(image.icc.is_none(), "declared size {declared_size} must be rejected");
            assert_eq!(image.dict.name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
        }
        assert_eq!(image.data, [90, 80, 70], "profile validation must not alter source samples");
    }
}
