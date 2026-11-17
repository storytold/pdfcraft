use super::*;

/// A gray "photo": `paper(x, y)` brightness with dark ink rectangles `[x, y, w, h]`.
fn photo(width: usize, height: usize, paper: impl Fn(usize, usize) -> u8, ink: &[[usize; 4]], ink_value: u8) -> Vec<u8> {
    let mut out = vec![0; width * height];
    for y in 0..height {
        for x in 0..width {
            let inside = ink.iter().any(|r| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]);
            out[y * width + x] = if inside { ink_value } else { paper(x, y) };
        }
    }
    out
}

fn rgb(gray: &[u8], width: usize, height: usize) -> RgbImage {
    RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let v = gray[y as usize * width + x as usize];
        image::Rgb([v, v, v])
    })
}

fn jpeg(image: &RgbImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// Photos keep the image signature limits (#1124): a 4 MiB file, at most 32 megapixels and
/// 8192 pixels per side decoded, and a working copy of at most 4 megapixels.
#[test]
fn photos_keep_the_image_signature_limits() {
    let png = |image: &RgbImage| {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    };
    assert!(matches!(SignaturePhoto::from_bytes(&vec![0; MAX_PHOTO_BYTES + 1]), Err(ScanError::Size)));
    assert!(SignaturePhoto::read(std::io::repeat(0)).is_err());
    assert!(matches!(SignaturePhoto::from_bytes(&png(&RgbImage::new(8193, 1))), Err(ScanError::Size)));
    assert!(matches!(SignaturePhoto::from_bytes(b"GIF89a\x02\0\x02\0"), Err(ScanError::Format)));
    let [w, h] = SignaturePhoto::from_bytes(&png(&RgbImage::from_pixel(8000, 4000, image::Rgb([220, 220, 215])))).unwrap().size();
    assert!(w <= WORKING_MAX_SIDE && u64::from(w) * u64::from(h) <= MAX_OUTPUT_PIXELS, "{w} x {h}");
    assert!((f64::from(w) / f64::from(h) - 2.0).abs() < 0.01, "the aspect ratio is kept: {w} x {h}");
}

fn region(r: [usize; 4]) -> Region {
    Region { x: r[0], y: r[1], width: r[2], height: r[3] }
}

fn opts() -> Options {
    Options { sensitivity: DEFAULT_SENSITIVITY, despeckle: true, background_radius: None }
}

#[test]
fn box_blur_averages_and_keeps_a_constant_signal() {
    let mut values = vec![0.0, 0.0, 9.0, 0.0, 0.0];
    box_blur(&mut values, 5, 1, 1);
    assert_eq!(values, vec![0.0, 3.0, 3.0, 3.0, 0.0]);
    let mut flat = vec![7.0; 12];
    box_blur(&mut flat, 4, 3, 2);
    assert!(flat.iter().all(|v| (v - 7.0).abs() < 1e-4));
}

#[test]
fn square_filters_match_a_naive_window_with_repeated_edges() {
    let (w, h) = (13, 7);
    let src: Vec<u8> = (0..w * h).map(|i| ((i * 37 + 11) % 251) as u8).collect();
    for radius in [1, 2, 5] {
        for max in [true, false] {
            let fast = square_filter(&src, w, h, radius, max);
            for y in 0..h {
                for x in 0..w {
                    let mut want = if max { 0 } else { 255 };
                    for dy in -(radius as i64)..=radius as i64 {
                        for dx in -(radius as i64)..=radius as i64 {
                            let sx = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                            let sy = (y as i64 + dy).clamp(0, h as i64 - 1) as usize;
                            let v = src[sy * w + sx];
                            want = if max { want.max(v) } else { want.min(v) };
                        }
                    }
                    assert_eq!(fast[y * w + x], want, "radius {radius} max {max} at {x},{y}");
                }
            }
        }
    }
}

#[test]
fn components_are_eight_connected() {
    let mask = [true, false, false, false, false, true, false, true, false, false, false, false];
    let (labels, count) = label_components(&mask, 4, 3);
    assert_eq!(count, 2);
    assert_eq!(labels[0], labels[5]);
    assert_ne!(labels[7], labels[0]);
    assert_eq!(labels[1], 0);
}

#[test]
fn sensitivity_lowers_the_threshold() {
    assert!(threshold(0) > threshold(50));
    assert!(threshold(50) > threshold(100));
    assert_eq!(threshold(200), threshold(100));
}

#[test]
fn ink_survives_a_shadow_gradient_and_the_shadow_does_not() {
    let (w, h) = (160, 80);
    let stroke = [20, 30, 120, 6];
    let gray = photo(w, h, |x, _| (235 - 115 * x / (w - 1)) as u8, &[stroke], 30);
    let ink = extract_ink(&gray, w, h, opts());
    assert_eq!(ink.alpha[10 * w + 150], 0);
    assert_eq!(ink.alpha[33 * w + 30], 255);
    assert_eq!(ink.alpha[33 * w + 130], 255);
    assert_eq!(ink.bounds, Some(region(stroke)));
}

#[test]
fn a_sharp_shadow_edge_is_not_ink() {
    let (w, h) = (200, 120);
    let stroke = [30, 52, 140, 4];
    let gray = photo(w, h, |_, y| if y < 60 { 230 } else { 120 }, &[stroke], 30);
    let ink = extract_ink(&gray, w, h, opts());
    for y in 58..63 {
        assert_eq!(ink.alpha[y * w + 10], 0);
        assert_eq!(ink.alpha[y * w + 190], 0);
    }
    assert_eq!(ink.bounds, Some(region(stroke)));
}

#[test]
fn a_table_at_the_edge_of_the_frame_is_not_ink() {
    let (w, h) = (300, 120);
    let stroke = [60, 60, 200, 4];
    let gray = photo(w, h, |x, y| if x < 12 || y < 10 { 70 } else { 225 }, &[stroke], 30);
    let ink = extract_ink(&gray, w, h, opts());
    assert_eq!(ink.alpha[5 * w + 150], 0);
    assert_eq!(ink.alpha[60 * w + 5], 0);
    assert_eq!(ink.alpha[11 * w + 150], 0);
    assert_eq!(ink.bounds, Some(region(stroke)));
}

#[test]
fn thick_strokes_stay_solid_when_the_window_is_sized_on_the_photo() {
    let (w, h) = (600, 200);
    let stroke = [50, 90, 500, 18];
    let gray = photo(w, h, |_, _| 225, &[stroke], 30);
    let middle = 99 * w + 300;
    // Sized on the tight frame alone, the window is narrower than the stroke.
    assert_eq!(extract_ink(&gray, w, h, opts()).alpha[middle], 0);
    let ink = extract_ink(&gray, w, h, Options { background_radius: Some((4.0 * w as f32 * BACKGROUND_RADIUS_RATIO) as usize), ..opts() });
    assert_eq!(ink.bounds, Some(region(stroke)));
    assert!((90..108).all(|y| ink.alpha[y * w + 300] == 255));
}

#[test]
fn faint_stroke_ends_touching_ink_are_kept_and_isolated_ones_are_not() {
    let (w, h) = (200, 60);
    let mut gray = photo(w, h, |_, _| 220, &[[20, 28, 80, 4]], 40);
    for y in 28..32 {
        for x in (100..180).chain(10..15) {
            gray[y * w + x] = 175;
        }
    }
    let ink = extract_ink(&gray, w, h, opts());
    assert!(ink.alpha[30 * w + 150] > 0);
    assert_eq!(ink.alpha[30 * w + 12], 0);
    assert_eq!(ink.bounds, Some(region([20, 28, 160, 4])));
}

#[test]
fn dust_is_removed_unless_despeckling_is_off() {
    let (w, h) = (200, 100);
    let gray = photo(w, h, |_, _| 230, &[[20, 40, 150, 5], [190, 5, 1, 1]], 20);
    assert!(extract_ink(&gray, w, h, Options { despeckle: false, ..opts() }).alpha[5 * w + 190] > 0);
    let ink = extract_ink(&gray, w, h, opts());
    assert_eq!(ink.alpha[5 * w + 190], 0);
    assert_eq!(ink.bounds, Some(region([20, 40, 150, 5])));
}

#[test]
fn faint_strokes_need_a_higher_sensitivity() {
    let (w, h) = (120, 60);
    let gray = photo(w, h, |_, _| 220, &[[10, 25, 100, 4]], 165);
    assert!(extract_ink(&gray, w, h, Options { sensitivity: 0, ..opts() }).bounds.is_none());
    assert!(extract_ink(&gray, w, h, Options { sensitivity: 100, ..opts() }).bounds.is_some());
}

#[test]
fn blank_paper_has_no_ink() {
    let gray = photo(64, 64, |x, y| (200 + (x * 7 + y * 13) % 20) as u8, &[], 0);
    assert!(extract_ink(&gray, 64, 64, opts()).bounds.is_none());
    assert!(detect_region(&gray, 64, 64).is_none());
}

#[test]
fn detection_frames_all_words_and_ignores_dust_and_the_table_edge() {
    let (w, h) = (400, 200);
    let gray = photo(w, h, |_, y| if y >= 185 { 30 } else { 225 }, &[[40, 95, 90, 5], [230, 92, 30, 5], [300, 20, 1, 1]], 30);
    let r = detect_region(&gray, w, h).unwrap();
    assert!(r.x <= 40 && r.x + r.width >= 260, "{r:?}");
    assert!(r.y > 20 && r.y + r.height < 185, "{r:?}");
}

#[test]
fn detection_frames_the_signature_not_the_sheet_on_a_dark_table() {
    let (w, h) = (400, 300);
    let sheet = |x: usize, y: usize| if (60..340).contains(&x) && (50..250).contains(&y) { 220 } else { 60 };
    let gray = photo(w, h, sheet, &[[120, 150, 140, 4], [150, 135, 4, 20]], 30);
    let r = detect_region(&gray, w, h).unwrap();
    assert!(r.x > 80 && r.x + r.width < 320, "{r:?}");
    assert!(r.y > 70 && r.y + r.height < 230, "{r:?}");
}

#[test]
fn rects_turn_with_the_photo_and_back() {
    let rect = PhotoRect { x: 10, y: 20, width: 30, height: 40 };
    let turned = rect.turned(Turn::Clockwise, 100, 80);
    assert_eq!(turned, PhotoRect { x: 20, y: 10, width: 40, height: 30 });
    assert_eq!(turned.turned(Turn::CounterClockwise, 80, 100), rect);
    assert_eq!(PhotoRect { x: 500, y: 0, width: 0, height: 900 }.clamped(100, 50), PhotoRect { x: 99, y: 0, width: 1, height: 50 });
    assert_eq!(PhotoRect { x: 0, y: 0, width: u32::MAX, height: 1 }.turned(Turn::CounterClockwise, 10, 10).y, 0);
}

#[test]
fn a_photo_is_scanned_into_a_trimmed_transparent_signature_in_its_ink_colour() {
    let (w, h) = (600, 400);
    let mut image = rgb(&photo(w, h, |x, _| (230 - 60 * x / w) as u8, &[], 0), w, h);
    for (x, y, p) in image.enumerate_pixels_mut() {
        if (150..450).contains(&x) && (200..206).contains(&y) || (200..206).contains(&x) && (170..206).contains(&y) {
            *p = image::Rgb([30, 50, 140]);
        }
    }
    let mut photo = SignaturePhoto::from_bytes(&jpeg(&image)).unwrap();
    let rect = photo.auto_frame();
    assert!(rect.x <= 150 && rect.x + rect.width >= 450 && rect.y <= 170 && rect.y + rect.height >= 206, "{rect:?}");
    let signature = photo.scan(rect, DEFAULT_SENSITIVITY, SCAN_MAX_SIDE).unwrap();
    let [sw, sh] = signature.size();
    assert!((300..340).contains(&sw) && (36..60).contains(&sh), "{sw}x{sh}");
    let pixels = signature.rgba().as_chunks::<4>().0;
    assert_eq!(pixels[0][3], 0, "the margin is transparent");
    let solid: Vec<_> = pixels.iter().filter(|p| p[3] == 255).collect();
    assert!(!solid.is_empty());
    assert!(solid.iter().all(|p| p[2] > p[0] + 40), "blue ink stays blue: {:?}", solid[0]);
    let quick = photo.scan(rect, DEFAULT_SENSITIVITY, PREVIEW_MAX_SIDE).unwrap();
    assert!(quick.size()[0] <= sw);
}

#[test]
fn a_sideways_signature_is_turned_upright() {
    let (w, h) = (400, 600);
    let gray = photo(w, h, |_, _| 225, &[[200, 120, 5, 300], [185, 300, 25, 4]], 30);
    let mut photo = SignaturePhoto::from_bytes(&jpeg(&rgb(&gray, w, h))).unwrap();
    let rect = photo.auto_frame();
    assert_eq!(photo.size(), [600, 400]);
    assert!(rect.width > rect.height * 2, "{rect:?}");
}

#[test]
fn an_empty_frame_reports_that_no_signature_was_found() {
    let photo = SignaturePhoto::from_bytes(&jpeg(&RgbImage::from_pixel(200, 100, image::Rgb([220, 220, 220])))).unwrap();
    let mut copy = photo.clone();
    assert_eq!(copy.auto_frame(), PhotoRect { x: 28, y: 35, width: 142, height: 30 });
    assert!(matches!(photo.scan(PhotoRect { x: 0, y: 0, width: 200, height: 100 }, 50, SCAN_MAX_SIDE), Err(ScanError::NoInk)));
}

#[test]
fn large_photos_are_downscaled_and_invalid_ones_rejected() {
    let photo = SignaturePhoto::from_bytes(&jpeg(&RgbImage::from_pixel(5000, 100, image::Rgb([200, 200, 200])))).unwrap();
    let [w, h] = photo.size();
    assert!((4000..=WORKING_MAX_SIDE).contains(&w) && (81..=82).contains(&h), "{w} x {h}");
    assert!(photo.thumbnail(512).width() <= 512);
    assert!(matches!(SignaturePhoto::from_bytes(b"not a photo"), Err(ScanError::Format)));
    assert!(matches!(SignaturePhoto::from_bytes(b"GIF89a\x02\0\x02\0"), Err(ScanError::Format)));
    assert!(SignaturePhoto::from_bytes(b"\xff\xd8\xff\xe0truncated").is_err());
    assert!(matches!(SignaturePhoto::from_bytes(&vec![0; MAX_PHOTO_BYTES + 1]), Err(ScanError::Size)));
    assert!(SignaturePhoto::read(std::io::repeat(0)).is_err());
    // A header claiming a huge image is rejected before decoding.
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR");
    png.extend_from_slice(&20_000_u32.to_be_bytes());
    png.extend_from_slice(&20_000_u32.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    assert!(SignaturePhoto::from_bytes(&png).is_err());
}

#[test]
fn degenerate_buffers_do_not_panic() {
    assert!(extract_ink(&[], 0, 0, opts()).bounds.is_none());
    assert!(extract_ink(&[0; 3], 2, 2, opts()).bounds.is_none());
    assert!(extract_ink(&[0], 1, 1, opts()).bounds.is_none());
    assert!(detect_region(&[255; 4], 2, 2).is_none());
    let mut empty: Vec<f32> = Vec::new();
    box_blur(&mut empty, 0, 0, 3);
    assert!(square_filter(&[], 0, 0, 2, true).is_empty());
    let photo = SignaturePhoto::from_bytes(&jpeg(&RgbImage::from_pixel(3, 2, image::Rgb([10, 10, 10])))).unwrap();
    let _ = photo.scan(PhotoRect { x: 9, y: 9, width: 0, height: 0 }, 255, 0);
}
