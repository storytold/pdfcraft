//! Synthetic multi-image regression; positions and barcode samples are independent of detection.
use crate as hayro;
use std::time::Instant;
const W: usize = 700;
const H: usize = 1000;
const BITS: &str = "1101001000010100011000100010110001100011101011";
/// Build the default synthetic multi-image page in the requested paint order.
fn pdf(order: &[usize]) -> Vec<u8> {
    pdf_sized(order, W, H)
}
/// Build ordered synthetic image occurrences with explicitly controlled source dimensions.
fn pdf_sized(order: &[usize], width: usize, height: usize) -> Vec<u8> {
    pdf_layered(order, width, height, false)
}
/// Build the sized fixture with optional-content wrapping for admission visibility checks.
fn pdf_layered(order: &[usize], width: usize, height: usize, layered: bool) -> Vec<u8> {
    pdf_content(order, width, height, layered, false)
}
/// Serialize bounded synthetic image occurrences, optional visibility and a cancellation-triggering text run.
/// Each source has a distinct exact key and independently placed Code128 bars.
fn pdf_content(order: &[usize], width: usize, height: usize, layered: bool, text: bool) -> Vec<u8> {
    let mut objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()];
    if layered {
        objects[0] = b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [18 0 R] /D << /OFF [18 0 R] >> >> >>".to_vec();
    }
    let resources = (0..13).map(|i| format!("/I{i} {} 0 R ", i + 5)).collect::<String>();
    objects.push(
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 4900 2000] /Resources << /Font << /F 19 0 R >> /XObject << {resources} >> /Properties << /Layer 18 0 R >> >> /Contents 4 0 R >>").into_bytes(),
    );
    let command = order.iter().map(|i| format!("q 700 0 0 1000 {} {} cm /I{i} Do Q\n", i % 7 * 700, 1000 - i / 7 * 1000)).collect::<String>();
    let command = if text { format!("BT /F 12 Tf (Cancel this pass) Tj ET\n{command}") } else { command };
    let command = if layered { format!("/OC /Layer BDC\n{command}EMC\n") } else { command };
    let mut content = format!("<< /Length {} >>\nstream\n", command.len()).into_bytes();
    content.extend(command.bytes());
    content.extend(b"endstream");
    objects.push(content);
    for i in 0..13 {
        if !order.contains(&i) {
            objects.push(b"null".to_vec());
            continue;
        }
        let mut data = vec![255; width * height];
        data[0] = i as u8; // distinct exact keys, outside every protected region
        for y in 200..280 {
            for (x, bit) in BITS.bytes().enumerate() {
                if bit == b'1' {
                    for dx in 0..3 {
                        data[y * width + 100 + x * 3 + dx] = 0;
                    }
                }
            }
        }
        let mut object = format!(
            "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n",
            data.len()
        )
        .into_bytes();
        object.extend(data);
        object.extend(b"\nendstream");
        objects.push(object);
    }
    if layered {
        objects.push(b"<< /Type /OCG /Name (Synthetic layer) >>".to_vec());
    }
    if text {
        if !layered {
            objects.push(b"null".to_vec());
        }
        objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).bytes());
    for offset in offsets.iter().skip(1) {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).bytes());
    out
}
/// Render the fixture at 0.4x, optionally selecting a fixed device-space tile with its own offset.
fn draw<'a>(p: &'a hayro::hayro_syntax::Pdf, c: &hayro::RenderCache<'a>, tile: Option<(usize, usize)>) -> Vec<u8> {
    let t = tile.unwrap_or((0, 0));
    hayro::render(
        &p.pages()[0],
        c,
        &Default::default(),
        &hayro::RenderSettings {
            x_scale: 0.4,
            y_scale: 0.4,
            width: tile.map(|_| 280),
            height: tile.map(|_| 400),
            x_offset: t.0 as f32,
            y_offset: t.1 as f32,
            ..Default::default()
        },
    )
    .data_as_u8_slice()
    .to_vec()
}
/// Exercise admitted page counts across reordered painting, cache priming/eviction and stitched tiles.
#[test]
fn image_order_cache_history_eviction_and_tiles_agree() {
    for count in [7, 11, 12, 13] {
        check(count);
    }
}
/// Compare cold/warm, reordered, primed and evicted renders plus tile assembly for one image count.
fn check(count: usize) {
    println!("images={count}");
    let all = (0..count).collect::<Vec<_>>();
    let reversed = (0..count).rev().collect::<Vec<_>>();
    let p = hayro::hayro_syntax::Pdf::new(pdf(&all)).unwrap();
    let q = hayro::hayro_syntax::Pdf::new(pdf(&reversed)).unwrap();
    let prime = hayro::hayro_syntax::Pdf::new(pdf(&[0])).unwrap();
    let c = hayro::RenderCache::new();
    let start = Instant::now();
    let cold = draw(&p, &c, None);
    println!("cold_ms={:.3}", start.elapsed().as_secs_f64() * 1000.);
    let reverse = draw(&q, &hayro::RenderCache::new(), None);
    let diff = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    let order_diff = diff(&cold, &reverse);
    println!("reordered_different_bytes={order_diff}");
    let hot = hayro::RenderCache::new();
    draw(&prime, &hot, None);
    let primed = draw(&p, &hot, None);
    let cache_diff = diff(&cold, &primed);
    println!("primed_different_bytes={cache_diff}");
    // Thirteen separately admitted pages exceed the cache's eight Mi-pixel cap, forcing
    // positive entries out before revisiting the original page.
    let eviction_pages = (0..13).map(|i| hayro::hayro_syntax::Pdf::new(pdf(&[i])).unwrap()).collect::<Vec<_>>();
    let eviction_cache = hayro::RenderCache::new();
    for page in &eviction_pages {
        draw(page, &eviction_cache, None);
    }
    let eviction_diff = diff(&cold, &draw(&p, &eviction_cache, None));
    assert_eq!(eviction_diff, 0);
    let mut tiled = vec![0; cold.len()];
    for i in 0..count {
        let tx = i % 7 * 280;
        let ty = i / 7 * 400;
        let tile = draw(&p, &hayro::RenderCache::new(), Some((tx, ty)));
        for y in 0..400 {
            let a = ((ty + y) * 1960 + tx) * 4;
            let b = y * 280 * 4;
            tiled[a..a + 1120].copy_from_slice(&tile[b..b + 1120]);
        }
    }
    let tile_diff = diff(&cold, &tiled);
    println!("tiled_different_bytes={tile_diff}");
    let start = Instant::now();
    let warm = draw(&p, &c, None);
    println!("warm_ms={:.3}", start.elapsed().as_secs_f64() * 1000.);
    let warm_diff = diff(&cold, &warm);
    println!("warm_evicted_different_bytes={warm_diff}");
    assert_eq!(order_diff + cache_diff + tile_diff + warm_diff, 0);
}

/// Count repeated image occurrences toward admission even when all draws share one cached source.
#[test]
fn image_occurrence_cap_is_independent_of_reusing_one_cached_image() {
    // Both pages fit the pixel cap. One more occurrence crosses only the 16-image cap.
    let admitted = hayro::hayro_syntax::Pdf::new(pdf_sized(&[0; 16], 512, 512)).unwrap();
    let declined = hayro::hayro_syntax::Pdf::new(pdf_sized(&[0; 17], 512, 512)).unwrap();
    let cache = hayro::RenderCache::new();
    assert!(crate::renderer::barcode_page_admitted(&admitted.pages()[0], &cache, &Default::default()));
    assert!(!crate::renderer::barcode_page_admitted(&declined.pages()[0], &cache, &Default::default()));
    let a = draw(&admitted, &cache, None);
    let b = draw(&declined, &cache, None);
    assert_ne!(a, b, "the over-budget page keeps the smoothed baseline throughout");
    assert_eq!(draw(&declined, &cache, Some((0, 0))), draw(&declined, &hayro::RenderCache::new(), Some((0, 0))));
}

/// Ensure a warm metadata decision avoids another interpreter pass for the same page/settings.
#[test]
fn repeated_admission_does_not_interpret_the_same_page_again() {
    let pdf = hayro::hayro_syntax::Pdf::new(pdf(&[0])).unwrap();
    let cache = hayro::RenderCache::new();
    let settings = Default::default();
    let page = &pdf.pages()[0];
    assert!(crate::renderer::barcode_page_admitted(page, &cache, &settings));
    assert!(crate::renderer::barcode_page_admitted(page, &cache, &settings));
    assert_eq!(cache.barcode_admission_passes.get(), 1);
}

/// Verify visibility/settings change admission identity, cancelled passes are not cached and large keys bypass memoization.
#[test]
fn admission_cache_tracks_visibility_cancellation_and_large_keys() {
    use std::sync::{Arc, atomic::AtomicBool};
    let pdf = hayro::hayro_syntax::Pdf::new(pdf_layered(&[0; 17], 512, 512, true)).unwrap();
    let cache = hayro::RenderCache::new();
    let page = &pdf.pages()[0];
    let mut settings = hayro_interpret::InterpreterSettings::default();
    assert!(crate::renderer::barcode_page_admitted(page, &cache, &settings));
    settings.ocg_overrides = Arc::new(vec![(18, 0, true)]);
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    settings.ocg_overrides = Arc::new(vec![(18, 0, false)]);
    assert!(crate::renderer::barcode_page_admitted(page, &cache, &settings));
    settings.ocg_overrides = Arc::new(vec![(18, 0, true)]);
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    assert_eq!(cache.barcode_admission_passes.get(), 3);
    settings.render_annotations = false;
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    settings.hide_comments = true;
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    settings.preview = true;
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    assert_eq!(cache.barcode_admission_passes.get(), 6);
    settings.cancelled = Some(Arc::new(AtomicBool::new(false)));
    for _ in 0..2 {
        assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    }
    assert_eq!(cache.barcode_admission_passes.get(), 6);
    settings.cancelled = Some(Arc::new(AtomicBool::new(true)));
    assert!(!crate::renderer::barcode_page_admitted(page, &cache, &settings));
    assert_eq!(cache.barcode_admission_passes.get(), 6);
    settings.cancelled = None;
    settings.ocg_overrides = Arc::new(vec![(99, 0, false); 257]);
    for _ in 0..2 {
        assert!(crate::renderer::barcode_page_admitted(page, &cache, &settings));
    }
    assert_eq!(cache.barcode_admission_passes.get(), 8);
    assert_eq!(cache.barcode_admission.borrow().len(), 6);
}

/// Exercise the sixteen-entry admission FIFO with distinct pages whose content bytes happen to match.
#[test]
fn admission_cache_eviction_is_bounded_and_keeps_page_identity() {
    let pages = (0..18).map(|_| hayro::hayro_syntax::Pdf::new(pdf(&[0])).unwrap()).collect::<Vec<_>>();
    let cache = hayro::RenderCache::new();
    for pdf in &pages {
        assert!(crate::renderer::barcode_page_admitted(&pdf.pages()[0], &cache, &Default::default()));
    }
    assert_eq!(cache.barcode_admission_passes.get(), 18);
    assert_eq!(cache.barcode_admission.borrow().len(), 16);
    assert!(crate::renderer::barcode_page_admitted(&pages[0].pages()[0], &cache, &Default::default()));
    assert_eq!(cache.barcode_admission_passes.get(), 19);
    assert_eq!(cache.barcode_admission.borrow().len(), 16);
}

/// Cancel during text interpretation and verify partial image counts cannot enter the admission cache.
#[test]
fn interruption_during_admission_never_caches_a_partial_count() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let pdf = hayro::hayro_syntax::Pdf::new(pdf_content(&[0; 17], 512, 512, false, true)).unwrap();
    let cache = hayro::RenderCache::new();
    let stop = Arc::new(AtomicBool::new(false));
    let request_stop = stop.clone();
    let settings = hayro_interpret::InterpreterSettings {
        cancelled: Some(stop.clone()),
        font_resolver: Arc::new(move |_| {
            request_stop.store(true, Ordering::Relaxed);
            None
        }),
        ..Default::default()
    };
    assert!(!crate::renderer::barcode_page_admitted(&pdf.pages()[0], &cache, &settings));
    assert!(stop.load(Ordering::Relaxed));
    assert!(cache.barcode_admission.borrow().is_empty());
    assert!(!crate::renderer::barcode_page_admitted(&pdf.pages()[0], &cache, &Default::default()));
    assert_eq!(cache.barcode_admission_passes.get(), 2);
}

/// Require identical pixels after cold/warm admission on nested forms near the shared paint limit.
#[test]
fn admission_miss_and_hit_leave_the_renderers_nested_paint_budget_equal() {
    let commands = (0..30_000).map(|i| format!("q 1 0 0 1 {} {} cm /A Do Q\n", i % 200 * 3, i / 200 * 5)).collect::<String>();
    let stream = |content: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes();
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 700 1000] /Resources << /XObject << /A 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream(&commands, ""),
        stream("/B Do", "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /B 6 0 R >> >>"),
        stream("0 0 1 1 re f", "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << >>"),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n", i + 1).bytes());
        bytes.extend(object);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).bytes());
    for offset in offsets.iter().skip(1) {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).bytes());
    let pdf = hayro::hayro_syntax::Pdf::new(bytes).unwrap();
    let cache = hayro::RenderCache::new();
    let cold = draw(&pdf, &cache, None);
    let warm = draw(&pdf, &cache, None);
    assert_eq!(cold.iter().zip(&warm).filter(|(a, b)| a != b).count(), 0);
}

/// Verify the actual viewer token reaches detection and a cancelled scan remains retryable.
#[test]
fn viewer_cancellation_reaches_decoder_without_poisoning_source_cache() {
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let pdf = hayro::hayro_syntax::Pdf::new(pdf(&[0])).unwrap();
    let cache = hayro::RenderCache::new();
    let stop = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(AtomicBool::new(false));
    let calls = Rc::new(Cell::new(0));
    let (hook_stop, hook_observed, hook_calls) = (stop.clone(), observed.clone(), calls.clone());
    crate::barcode::with_test_decoder(
        move |cancel| {
            hook_calls.set(hook_calls.get() + 1);
            if hook_calls.get() == 1 {
                hook_stop.store(true, Ordering::Relaxed);
                hook_observed.store(cancel.load(Ordering::Relaxed), Ordering::Relaxed);
                Some(Err("cancelled".into()))
            } else {
                None
            }
        },
        || {
            let settings = hayro_interpret::InterpreterSettings { cancelled: Some(stop), ..Default::default() };
            let _discarded =
                hayro::render(&pdf.pages()[0], &cache, &settings, &hayro::RenderSettings { x_scale: 0.4, y_scale: 0.4, ..Default::default() });
            assert!(observed.load(Ordering::Relaxed), "production decoder must receive the active viewer token");
            let _next = draw(&pdf, &cache, None);
            assert_eq!(calls.get(), 2, "an aborted scan must not become a permanent negative cache entry");
        },
    );
}

/// Compare errors, deadlines and caught panics with ordinary smoothing, including warm negative-cache reuse.
#[test]
fn decoder_faults_keep_smoothed_fallback_and_are_cached() {
    use std::{cell::Cell, rc::Rc};
    let pdf = hayro::hayro_syntax::Pdf::new(pdf(&[0])).unwrap();
    let expected = crate::barcode::with_test_decoder(|_| Some(Ok(Vec::new())), || draw(&pdf, &hayro::RenderCache::new(), None));
    for kind in ["unexpected error", "deadline", "panic"] {
        let cache = hayro::RenderCache::new();
        let calls = Rc::new(Cell::new(0));
        let hook_calls = calls.clone();
        crate::barcode::with_test_decoder(
            move |_| {
                hook_calls.set(hook_calls.get() + 1);
                if kind == "panic" {
                    panic!("injected decoder panic");
                }
                Some(Err(kind.into()))
            },
            || {
                assert_eq!(draw(&pdf, &cache, None), expected, "{kind}");
                assert_eq!(draw(&pdf, &cache, None), expected, "{kind} cached");
                assert_eq!(calls.get(), 1, "{kind} must not repeat decoding");
            },
        );
    }
}

/// Contributor-original retail symbol from fixed GS1 digit patterns; truth never uses decoder points.
pub(crate) fn retail_fixture(upc: bool, extended: bool) -> (Vec<u8>, crate::barcode::Rect, crate::barcode::Rect) {
    let left = ["0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011"];
    let parity = ["LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL"];
    let digits = if upc { "0012345678905" } else { "5901234123457" };
    let bytes = digits.as_bytes();
    let mut bits = String::from("101");
    for (i, kind) in parity[(bytes[0] - b'0') as usize].bytes().enumerate() {
        let pattern = left[(bytes[i + 1] - b'0') as usize];
        if kind == b'L' {
            bits.push_str(pattern);
        } else {
            bits.extend(pattern.bytes().rev().map(|v| if v == b'0' { '1' } else { '0' }));
        }
    }
    bits.push_str("01010");
    for &digit in &bytes[7..] {
        bits.extend(left[(digit - b'0') as usize].bytes().map(|v| if v == b'0' { '1' } else { '0' }));
    }
    bits.push_str("101");
    assert_eq!(bits.len(), 95);
    let mut data = vec![255u8; W * H];
    for (i, bit) in bits.bytes().enumerate() {
        if bit != b'1' {
            continue;
        }
        let guard = i < 3 || (45..50).contains(&i) || i >= 92 || (upc && (!(10..85).contains(&i)));
        let end = 280 + if extended && guard { 15 } else { 0 };
        for y in 200..end {
            for x in 100 + i * 3..103 + i * 3 {
                data[y * W + x] = 0;
            }
        }
    }
    // Scanned lettering outside the independent quiet rectangle must remain smoothed.
    for y in 500..530 {
        for x in (450..600).step_by(7) {
            data[y * W + x] = 0;
        }
    }
    let ink = crate::barcode::Rect { x0: 100, y0: 200, x1: 385, y1: if extended { 295 } else { 280 } };
    let quiet = crate::barcode::Rect { x0: 64, y0: 196, x1: 421, y1: ink.y1 + 4 };
    (data, ink, quiet)
}

/// A fixed contributor-original version-2 QR matrix, with independently specified four-module quiet zone.
pub(crate) fn qr_fixture() -> (Vec<u8>, crate::barcode::Rect, crate::barcode::Rect) {
    const ROWS: &[&str] = &[
        "1111111011110101001111111",
        "1000001010111100001000001",
        "1011101000101001101011101",
        "1011101010001011101011101",
        "1011101000001011001011101",
        "1000001000111100101000001",
        "1111111010101010101111111",
        "0000000010011001100000000",
        "1011011100010100001001011",
        "0111000110101111111100001",
        "1101101101001100110110100",
        "0011010010000000001011100",
        "0000111001001010001011011",
        "0010010101111011001010010",
        "0111111011110100100101010",
        "1011100011010010010100001",
        "0000001010101010111111010",
        "0000000010000011100010110",
        "1111111011000111101011111",
        "1000001010110111100011000",
        "1011101000100110111110100",
        "1011101010000000001101011",
        "1011101011100010011111010",
        "1000001001011111111111100",
        "1111111010110000011101011",
    ];
    let mut data = vec![255u8; W * H];
    for (y, row) in ROWS.iter().enumerate() {
        for (x, v) in row.bytes().enumerate() {
            if v == b'1' {
                for yy in 200 + y * 4..204 + y * 4 {
                    for xx in 100 + x * 4..104 + x * 4 {
                        data[yy * W + xx] = 0;
                    }
                }
            }
        }
    }
    for y in 500..530 {
        for x in (450..600).step_by(7) {
            data[y * W + x] = 0;
        }
    }
    (data, crate::barcode::Rect { x0: 100, y0: 200, x1: 200, y1: 300 }, crate::barcode::Rect { x0: 84, y0: 184, x1: 216, y1: 316 })
}

/// Wrap synthetic bytes in one image PDF without fonts, external assets or fixture-derived decoder geometry.
fn raster_pdf(data: &[u8], width: usize, height: usize, filter: &str, placement: &str, page: (usize, usize)) -> Vec<u8> {
    let mut image=format!("<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 {filter} /Length {} >>\nstream\n",data.len()).into_bytes();
    image.extend(data);
    image.extend(b"\nendstream");
    let command = format!("q {placement} cm /I Do Q");
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << /I 5 0 R >> >> /Contents 4 0 R >>", page.0, page.1)
            .into_bytes(),
        format!("<< /Length {} >>\nstream\n{command}\nendstream", command.len()).into_bytes(),
        image,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).bytes());
    for offset in offsets.iter().skip(1) {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).bytes());
    out
}

/// Validate QR/EAN/UPC regions and protected render samples against independently generated fixture bounds.
#[test]
fn positive_qr_and_retail_regions_and_rendering_match_independent_truth() {
    use std::sync::atomic::AtomicBool;
    for (format, data, ink, quiet) in [
        {
            let (d, i, q) = qr_fixture();
            ("QR_CODE", d, i, q)
        },
        {
            let (d, i, q) = retail_fixture(false, true);
            ("EAN_13", d, i, q)
        },
        {
            let (d, i, q) = retail_fixture(true, true);
            ("UPC_A", d, i, q)
        },
    ] {
        let rgba = data.iter().flat_map(|v| [*v, *v, *v, 255]).collect::<Vec<_>>();
        let regions = crate::barcode::detect(W, H, &rgba, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(regions.len(), 1, "{format}: {regions:?}");
        let region = &regions[0];
        assert_eq!(region.format, format);
        assert_eq!(region.ink, ink, "{format}");
        assert!(region.quiet.contains(quiet), "{format}: {region:?}");
        let pdf = hayro::hayro_syntax::Pdf::new(raster_pdf(&data, W, H, "", "700 0 0 1000 0 0", (W, H))).unwrap();
        let settings = hayro::RenderSettings { x_scale: 1.7, y_scale: 1.7, ..Default::default() };
        let pix = hayro::render(&pdf.pages()[0], &hayro::RenderCache::new(), &Default::default(), &settings);
        let buf = pix.data_as_u8_slice();
        let dw = pix.width() as usize;
        let scale = f64::from(settings.x_scale);
        for y in ((quiet.y0 as f64 * 1.7).ceil() as usize + 1)..((quiet.y1 as f64 * 1.7).floor() as usize - 1) {
            for x in ((quiet.x0 as f64 * 1.7).ceil() as usize + 1)..((quiet.x1 as f64 * 1.7).floor() as usize - 1) {
                let sx = ((x as f64 + 0.5) / scale).floor() as usize;
                let sy = ((y as f64 + 0.5) / scale).floor() as usize;
                assert_eq!(buf[(y * dw + x) * 4], data[sy * W + sx], "{format} protected pixel {x},{y}");
            }
        }
        assert!((850..900).any(|y| (760..1020).any(|x| matches!(buf[(y * dw + x) * 4], 1..=254))), "{format}: text must remain smooth");
    }
}

/// Serialize only contributor-original PDF objects, retaining fixed object numbers in regression fixtures.
fn objects_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).bytes());
    for offset in offsets.iter().skip(1) {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).bytes());
    out
}
/// Wrap contributor-original bytes in a PDF stream with an exact Length and supplied dictionary entries.
fn fixture_stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend(data);
    out.extend(b"\nendstream");
    out
}
/// Build sixteen small, distinct synthetic image sources without crossing the declared-pixel cap.
fn budget_pdf(order: &[usize]) -> Vec<u8> {
    let resources = (0..16).map(|i| format!("/I{i} {} 0 R ", i + 5)).collect::<String>();
    let commands = order.iter().map(|i| format!("q 64 0 0 64 {} 0 cm /I{i} Do Q\n", i * 64)).collect::<String>();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1024 64] /Resources << /XObject << {resources} >> >> /Contents 4 0 R >>").into_bytes(),
        fixture_stream("", commands.as_bytes()),
    ];
    for i in 0..16 {
        let mut data = vec![255; 64 * 64];
        data[0] = i as u8;
        objects.push(fixture_stream("/Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceGray /BitsPerComponent 8", &data));
    }
    objects_pdf(&objects)
}
/// Verify per-image deadline shares remain page-bounded across order, tile and warm-cache histories.
#[test]
fn multi_image_deadlines_are_page_bounded_independent_of_order_tiles_and_history() {
    use std::{cell::RefCell, rc::Rc};
    let order = (0..16).collect::<Vec<_>>();
    let pdf = hayro::hayro_syntax::Pdf::new(budget_pdf(&order)).unwrap();
    let reverse = hayro::hayro_syntax::Pdf::new(budget_pdf(&order.iter().copied().rev().collect::<Vec<_>>())).unwrap();
    let costs = Rc::new(RefCell::new(Vec::new()));
    let hook = costs.clone();
    crate::barcode::with_test_decoder(
        move |_| {
            hook.borrow_mut().push(crate::barcode::test_scan_millis());
            Some(Err("deadline".into()))
        },
        || {
            let settings = hayro::RenderSettings { x_scale: 0.5, y_scale: 0.5, ..Default::default() };
            let cache = hayro::RenderCache::new();
            let full = hayro::render(&pdf.pages()[0], &cache, &Default::default(), &settings);
            let first = costs.take();
            assert_eq!(first.len(), 16);
            println!("virtual_page_scan_ms={}", first.iter().sum::<u64>());
            assert!(first.iter().sum::<u64>() <= 1500, "16 independent deadlines exceed page allowance: {first:?}");
            let _warm = hayro::render(&pdf.pages()[0], &cache, &Default::default(), &settings);
            assert!(costs.borrow().is_empty());
            let reversed = hayro::render(&reverse.pages()[0], &hayro::RenderCache::new(), &Default::default(), &settings);
            assert_eq!(first, costs.take());
            assert_eq!(full.data_as_u8_slice(), reversed.data_as_u8_slice());
            let tile = hayro::render(
                &pdf.pages()[0],
                &hayro::RenderCache::new(),
                &Default::default(),
                &hayro::RenderSettings { width: Some(32), height: Some(32), ..settings },
            );
            let tile_costs = costs.take();
            assert!(!tile_costs.is_empty() && tile_costs.len() <= 16 && tile_costs.iter().all(|n| *n == first[0]));
            for y in 0..32 {
                assert_eq!(&full.data_as_u8_slice()[y * 512 * 4..y * 512 * 4 + 128], &tile.data_as_u8_slice()[y * 128..(y + 1) * 128]);
            }
            let prime = hayro::hayro_syntax::Pdf::new(budget_pdf(&[0])).unwrap();
            let primed = hayro::RenderCache::new();
            let _prime = hayro::render(&prime.pages()[0], &primed, &Default::default(), &settings);
            costs.borrow_mut().clear();
            let _next = hayro::render(&pdf.pages()[0], &primed, &Default::default(), &settings);
            assert_eq!(costs.take(), first, "negative entries must include the deterministic work allowance");
        },
    );
}
/// Force nested-paint truncation before later images and refuse incomplete admission metadata.
#[test]
fn incomplete_nested_paint_traversal_is_declined_and_never_cached() {
    let busy = "/A Do\n".repeat(30_000);
    let later = "/C Do\n".repeat(17);
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 700 1000] /Resources << /XObject << /Big 5 0 R /Later 8 0 R >> >> /Contents 4 0 R >>".to_vec(),
        fixture_stream("", b"/Big Do /Later Do"),
        fixture_stream(
            "/Type /XObject /Subtype /Form /BBox [10000 10000 10001 10001] /Group << /S /Transparency /I true /CS /DeviceCMYK >> /Resources << /XObject << /A 6 0 R >> >>",
            busy.as_bytes(),
        ),
        fixture_stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /B 7 0 R >> >>", b"/B Do"),
        fixture_stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << >>", b"0 0 1 1 re f"),
        fixture_stream("/Type /XObject /Subtype /Form /BBox [0 0 700 1000] /Resources << /XObject << /C 9 0 R >> >>", later.as_bytes()),
        fixture_stream("/Type /XObject /Subtype /Form /BBox [0 0 700 1000] /Resources << /XObject << /I 10 0 R >> >>", b"700 0 0 1000 0 0 cm /I Do"),
    ];
    objects.push(fixture_stream(
        "/Type /XObject /Subtype /Image /Width 512 /Height 512 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &vec![255; 512 * 512],
    ));
    let pdf = hayro::hayro_syntax::Pdf::new(objects_pdf(&objects)).unwrap();
    let cache = hayro::RenderCache::new();
    assert!(!crate::renderer::barcode_page_admitted(&pdf.pages()[0], &cache, &Default::default()), "truncated image counts cannot admit a page");
    assert!(cache.barcode_admission.borrow().is_empty());
}

/// Decode the contributor-original, lossless JPX fixture; its barcode samples were generated directly.
fn jpx_fixture() -> Vec<u8> {
    let hex = concat!(
        "ff4fff510029000000000834000008340000000000000000000008340000083400000000000000000001070101ff52000c00000001000504040001ff",
        "5c00134040484850484850484850484850484850ff640025000143726561746564206279204f70656e4a5045472076657273696f6e20322e352e34ff",
        "90000a000000001cf00001ff93efe1795f6835f6835f6818cff286f1aff72542ee036acc2f7fc8c04b6d04ea808443bcb56b45a5912baf6e7aa65fa9",
        "ff046a0dd928014c79995ccfe267bd749892ef124f794000061212ff7cc74407f3792902e3d188ed8413680309097fca89020c30751bc000008872ec",
        "41be02fe4bedbcb32d5c2cbadbfc13fae9c00003392c0352b985b0b7e697e1acae0c9289823c2ab712825fff53bbc8b68127377ee853b6b81a10c0b5",
        "f825f828f64c01429e7a4f63f917ff6861d8d78387abd7dfabf005f4e8b0ff5db81e892c5bf0b46ae7070c551f9ad68226ff7bcba20853fb2e3d00fb",
        "25db6fa704755ac51e219f5fd9a7b947f02c073249c9a4aad0c914005cafdd7f01d9057fdcdf01cabfefe7460c7ed4518fe0a400e07b29c45880a81e",
        "8eabea12a12817eca6f3cea5bf776f77ecab1e6f724f573e0a6abb7059dc88d356550f32852c5648bb34453e0a6746530f15c32502f0a7f3e8dea39f",
        "5971a6154cf01e1b5e16a8fc71bda1cceb44e4a252d57b3beedc1d3c178aeff5ae3a1196ede83f7cff2e9705671a1a6ac41d9b95547c897af6281b8d",
        "3c13ecc0eed4c8ced577edbfe47b41b662ed9bb948564a906c8866eae5e1ee183a29ef5a5a85d0d11d6b3e36983e9069238928cec7a0b3eae2955a4e",
        "28408aaa23f24ffe867f11e54733fedbaf45c8cccfe4aea67d6db105b972ab46fdf620941e4b6e63278b5365a1b316d111185e93f166932341ee362a",
        "b65396fed20a31c5995c725483613a0dcd965ec946efec7ec05454678c5459f03a0ba871c0deaea5bd695ff3fc35180e7f8510070feda040ed9264c5",
        "c8ea9c47c20ca46227c103d54da06164fcede0d91f532eff815a1b8e18bdbb4bd458bdc3fd49e17acb3ab125ed5c65ff68b860de11c556adf9b55af9",
        "49fc0000711c2aec5b4c0854333878819021c5dbe66c21acf61f4801d4252851376f9bfacdbc7e529127c87455ca7a4b96274a132c2f35c2e9240355",
        "76e10e84101a1a63ccf84538507222daf106329fbc24e30c3b585ec3a3c980716a70161876b0bd87479c745ec139513a0afa3afa0a5a2637efc7ce83",
        "b3cd56e30ba069b5eb0166f94b73e8161df609af580b37ca5b9f40b0c3b94f9630f977c7698031461129d1f43e5fe7bd5ccd7bda7d9d438ae3664a58",
        "21dd05d5b41f31ab6d99296087741756a5d9985213e1a4abf543de3e1389428adb0b71f7406b26492715e2396182121d03cc4746af139c5d280e8f26",
        "03355dcf2f42aee8dd1ebe4c06a109fc6af93c4c6d8fee23fbdafe9ae065dda60377be5f5fc9fcaa07246ec200133fe2f24c7497724ff25241e0f8ed",
        "0c52853d41a7af0a6cbed30df30ae8822c1e36770b16b772c8b6c7a528b9199d1118b1164b3c5487bf2770c1a8d621467a4d95d4db4febd270df460a",
        "ef44b87bcbe55a07adac8e90b40d873f1f1531cb2f9a597eb97087bc462b859bca3eb4deb47fed8ec513857645e6930a4121602557b55d4442e6a532",
        "2ec753b11983f2ab1dbc66bd0e9af38d8ec636a0f639f067e7cd1d320866254c97154bdaf90b2b89f052347813812fcd39a73d3a48ddbad4191a6a65",
        "2b874fdbd4f620fea41a741b7ab56e0b07885db4ac1f831e9f1ec5a8f669aeef779a1f120c15c8ed5215ad5f1e403ff9ff0531fc3458fe146ff0d050",
        "f3fe051fc130fe04ff609e1e1fed24fda847ed24fda860f6a390c050af91247cc532915fff7fc667cbb72bc2e4515f10b576a1801d43007119225b5e",
        "afea7a98d7abfac64d3c2fb7070ba3ff3a00778abbf1bdd7109742a8331aee494b4e6095e35f304490664ff72ca22acd77ea3152415dfa8c5490577e",
        "a31888756e4ac1960b81f45ec2687d1561f45ec2687d1561f45ec2684dcc30ddf1bf4ee71f45ec2687d1561f45ec2687d1561f45ec267ff5cd20c0a8",
        "7c512c3656fa34df870197233a09ff7fff7fff4f26313da82192ed2800cbb325603de0114f7a889369e0802c2a60ea009f8829ed00e06a009f8829ed",
        "00e06a009f88148bf5d3846b7bff41bfd62fa8a4509cbfff37bdf0457f2b9d2a5b029207091eb09a04cd7eca708c8ac27f75809747f1613e6369d737",
        "f1613e6369d722663429907658f59d8777e319458e80681548c3e28689b11cbe5737640aa461f14344d88e5f2b9bb2055230f8a1a26c47839837b1f6",
        "14f7a44f03015f88cc0bf0c6461f254408c231c1a87c66063230f92a2046118e0d43e330319187c951023094c04c31c6a171c31b0f75fe194448cc0b",
        "f0c6461f254408c231c1a87c66063230f92a2046118e0d43e330319187c951022f9e895b6037ddba9b67a4d32e458a5dc11afce946666ef9ee202feb",
        "d167fe714c2ba4e57803d4695651473a23237295ef0fb60587dc28b1dc1cb518f5578d7cc119072ee856348cb8a08233a5d3581f2f944135d157742e",
        "58713b4618a00326c80d701c41aab8ab0fa2ac3e8bd84d07d274c3129c59e2af57fc9c1f45ec2687d1561f45ec268037a465949e00221cf681b77400",
        "b737f74174caf54716a4380e32456a7de503f81d07d0bd7b70ed9158c41eb3c080f7ff0ffb84950d85f871b2f7f863c49c7f20a7b40381a8027e20a4",
        "9fa92316a7192ef3b0c34d788e4e067cf0dc8f400a4009719dd774e03a536f7c6bdbc5b73ae6fe2c27cc6d3ae4bbe8403e70e4733fddc0a44e3d9411",
        "21a4d59b8d89c657c4acf82782de055230f8a1a26c472f95cdd902a9187c50d1361e4239bcbf8060c5b01e6781a6a29296631ccadb605ec2fc319187",
        "c95102308c706a1f19818c8c3e4a881181ea4d77115e6c98f6fae5d9c1a4099b548c6e9e78c66063230f92a2046118e0d43e330319187c951022e01d",
        "42fd757f15cc717e178c348de90e3ff6aabb76198280dc7c2876055228afcdd517f6964040b07719378d22a0624574333e6540c93aa27ff5d441837c",
        "905ac4fd1d47dbe1c61fb088ad9151892e2890609e85406e89ee64eaf7e3a478c4abb9e01b9e87b7be3fa740aea7dd12e3ef205882236e2f5e51511d",
        "dcb6291f8adacc1c394abcda5eda424569734e72599f4f49eb8f12b5442d7009fe42b0e110112e5dec786925bb508638cdbfcdd6afc3be8c16200fb9",
        "e0196e1b82cdd0141cc05517ece47411a360add99ae745cac9c4d39dff01d936852caedcf37b3f4cc71aafbaf84e02a445399709ff34939498226b4e",
        "922186e0740f05c08ff6a0dcb753c2588d7b600c8fcc01c4dcd7dadce76052402fd0b285d5a0b0ff6581136beff5ca6cb7b3f91ebf43f4e53ee27b86",
        "e07d163ba3cc70df207ee3c302c3cdff01bbbf93001ffdc63ed46fb31ae939900d456570e017a46eea551dab6fd72733546abfd9d905448201285efc",
        "e30f96c0a4f0c9547a6920dd14eb96b6aba170bc49271cf107527fcd514550871f2ba6bccef900a3ae167ff8a06108c333761581bbf8cb16afbd2493",
        "9881a723796fb12710438940bae31f4042cb062dab2511c220b07503b606d5183d3ff904ff61a97fe1d04c7ce2aff0d15ff86d447ce2403c827fe067",
        "fe0b13ea07bfc0cffe0c13ea0700f2087fda57fda7d1f205bfb4abfda7b1f20500f2f58df4e516cee930355f6ea2187ffe6c6d163dd70dd7c49f0c38",
        "f06e4a56612e5b3ed13ce0bd4210cc03f72099237d1c11ff7479900305ae4c21c453a0f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6",
        "f9be6f9be6f9be6f9be6fb451be4c5b84259fc92853c1fb0a234499392a4a92a24c9c95254951264e4a92a4a8932725495254499392a4a929e9242fa",
        "d13921a31c948e732a3d192a51403b1e329794bca5e51234d187ca5e52f2891a68c3e52f2979448d3461f29794bca2469a30f94bca5e51234d187ca5",
        "e52f1c864eafdeef07618d431a86350c6b57e329794bca5e51234d187ca5e52f2891a68c3e52f2979448d3461f29794bca2469a30f94bca5e51234d1",
        "87ca5e52f2863f62413461f29794bca2469a30f94bca5e51234d187ca5e52f2891a68c3e52f2979448d3461f29794bca2469a30f94bca5e51234d187",
        "ca5e52f1f26cadf4e6274f0bb387cf2f0b29a483592653ff7f3cd9dab8720f1ab1ae0b522c7ece1b929597d10823825f2fb1d04045c360f84dcec47a",
        "d546d0ebbc4b94ead845bfff7819f00095e2b515c3683f0d4350a741f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37c",
        "df37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf7ca62f4f702227d77a975d906293273f927b0bf9ec7acc9525495254952549525495",
        "2549525495254952549525495254952549525495254952549525495254952549525495254952549525495254952547d7817d584b93ba7de24b91f872",
        "d1342a3feea75d247e2d9ed0c57146b194bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f",
        "29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bc7601d9ffe995d7c472a1b6a18d431a86350c6a18d431b2238a358ca5e52f2",
        "9794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f2979",
        "4bca5e52f29794bca5e538e41d07117e5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794",
        "bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f2978f255e202fcb7632142638fa265b5a9",
        "57b0494330afbfff63dd89e63de639892d20bab66383d48dc4cc76781b1d10c32679701a7ba515b0f0805ad89620b82b6ebe6f9be6f9be6f9be6f9be",
        "6f9be6f9be6f9be6f9be6f9be6f9be6fad196efacfcc2a678ab2ac89eacab2acab227ab2acab2ac89eacab2acab227ab2acab2ac33aa4099550fb297",
        "94bca5e51234d187ca5e52f2891a68c3e52f2979448d3461f29794bca2469a30f94bca5e3d7e0494b2b65678e2939c80cc02c029d249931785187ca5",
        "e52f2891a68c3e52f2979448d3461f29794bca2469a30f94bca5e51234d187ca5e52f252d49dd3ba77573f48d5187ca5e52f2891a68c3e52f2979448",
        "d3461f29794bca2469a30f94bca5e51234d187ca5e52f2792abb13a32c0dc2a72c5a0a3869b641af5fdda93ccf971787ff008b548f2c798a472dfd56",
        "350b2d2f718f91c04f182466dc7036464fded99557a1c82c55c0831b493961751b1795cccd8cfb1243899cbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbc",
        "bcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcb861c74c79cb0c6a84e10f4d2d4686678ab2acab2acab2acab2acab2acab2acab2acab2acab2acab",
        "2acab2acab2acab2acab2acab2acab2acab2acab2acab111290777baf6f568063d1ac652f29794bca5e52f29794bca5e52f29794bca5e52f29794bca",
        "5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f23ce6ad95b2b656cad95b2ba09203f461b162e55c26042e0be",
        "6bc3c329794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e",
        "52f29794bc6a4ee9dd3ba774ee9dd3baba10529194bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794bca5e52f29794",
        "bca5e52f29794bca5e52f29794bca5e52f29794bca5e51d31fb13a32c0dc2a9247cf976b141b3cf0ee12fc1dbdf1dccd9a4bebe83095f3e6b3aff3d1",
        "f4e0ba68e3fc71e9c2f913d5de3338a8e24f79454303223a6ffbf2a3d396e9df67e66f5ff153ed9e902fd84c82a2cab48d39b5626ffaa0bdf07cdd78",
        "68c70ea0ccaa7c65be592fe719f8a35bb820eefa897067993dd1ece58b5682ed5486a2789e7287ed051e1452f75deac6c8f8c6d423421615df401899",
        "016b307ff1341525eeb77fea25b3bd109c55364f1512ff79539464bbf529099a54184beb3f6d6a5c360b4c5df051fb24d1519306f911bf3b16af0388",
        "cfbfe832f3c719dba4909b2dfc540b160b05872fbbf3f03a750d18e1d3551f43b52c33c59b23d9cbe9a8d161c3c5d4bc72dda1dc6b5569c9aa3ee96f",
        "c7065a7fd8af64eb5d3ff85c0c2ad52b6155771f896ba8069bbe2eed31c841c487316a319d09e7beedc3b5fb7ff1d35b03e65defc11c015b62a191ff",
        "7fff6d86bb28231aba02c0c1383e819a0bff7fff74834817ba86f08ff14a7b03eee1f04b2a50de9f72118a4d78c64e69feaa289647bc541cb885447d",
        "c38541d8b05960d43983cf0a0dfbe9cfff7cc5f968982eba7ab8e018483ff133c023b3ea072427bf1cfeeb0dff4b8407fd8e92da69527b1afe0ac373",
        "abe0fca8c13ffa830428becf779064f8bfe8146428133d126c1ee2048021822863f28ffb30b2670a0f307ad951e3955c3d1f20e704f2c198a5c880fb",
        "7ee42fff4b09c6679762b0107a20c9f1ad6be7c19ca9bff90484ff70de8ff86c9ff0d93aff4362bff86a27f0d3f0007904913ff8107fc077f80fc7e7",
        "0aaff8187ff02ffe0638fce140007904843ff6887fb43bf68757fb46bff68b7ed1600000ce79e2e9461ecaec41ff7ffa4b37607dabd655ebdfac4d07",
        "c5414725d1281d00c004586ca7abff76dd2b2c9d98ddb1e29100196400b01f5cf56dd7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37",
        "cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf36e3e5a6552600004",
        "d3b06c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d",
        "86c361b0d86c361b0d86c361b0d86c361b0d91ab4ac316b75f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf",
        "37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37ce49643be2d6ebe6f9be6f9be6f9",
        "be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9",
        "be6f9be6f9be6f9be6f9be6f9be6f9c94b57785add7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cd",
        "f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cf8ae56b5b083981ff7ee81e07f0e050",
        "8eb12b56e0497e92dec84e8060022c3653d5ff7362f5578b8263d9040032c801617a2d6ebe6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6",
        "f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6dbdb2e632b00047f4df37cdf37cdf37cdf37cdf37cdf3",
        "7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37ce52624f62b6eb",
        "e6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b",
        "e6f9be6f9c971bc18adbaf9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be",
        "6f9be6f9be6f9be6f9be6f9be6f9be722a220c56dd7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cd",
        "f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37fcf8ae56b5b083981ff7ee81e07f0e050d1272356e024f32ad1306e2018008b0d94",
        "f57ff9d5636434a476ac8200196400b0bd16b75f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf",
        "37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf36e3d5dc2d3c0008fe9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b",
        "e6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9ca6fd21e2b6ebe6f9be6f9be6f9be6f9be6f9be6f9be6f",
        "9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9c9639418adbaf9be6f9be6f9",
        "be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be729",
        "adf422b6ebe6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b",
        "e6f9be6f9be6f9be6f9b369d6a890bf243f97feeca3a212f94caad958b0882231b253a6c7bb9682565b8381fff6646cda241b65380075875b0765bff",
        "7b9d7004d0032c801603f0adbaf9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be",
        "6f9be6f9be6f9bdffa1b0d86c5f40b54361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d8",
        "6c361b0d86c361b0d86c361b0d86c35a2d2000032c1f60d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d8",
        "6c361b0d86c361b0d86c361b0d86c361b0d86c361b0d88f134f85add7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf",
        "37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37ce01669fc56dd7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf3",
        "7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdfff7f4b21ef22fafec7ff7a96cbfd050d14ab",
        "69af9afaa00aa90f1fd9859dc07ffa92056f523948050465b0765bff340c27ea00659002c07e15b75f37cdf37cdf37cdf37cdf37cdf37cdf37cdf37c",
        "df37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37bff4361b1e447dfbe6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b",
        "e6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b57100013f0c04d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b",
        "0d86c361b0d86c361b0d86c361b0d86c477381456dd7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37c",
        "df37cdf37cdf37cdf37cdf38051edc2b6ebe6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9b",
        "e6f9be6f9be6f9bf4b21ef22fafec7ff7a96d03df4cd14ab6a53550619fe2715f0035bab583fff6f864b0271520141196c1d96ff723418e880196400",
        "b01f856dd7cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdf37cdeffd0d86c66446d0b61b0d86c361",
        "b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361ad1690004dfa201b0d86c361b0d86c361b0d86c",
        "361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d86c361b0d88ef7b80adbaf9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be",
        "6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be700adb222b6ebe6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6",
        "f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9be6f9bfb84899d0cfbb70c7044333491448d6ce636793f8f96f903f7de6efbdff7fff7fbc",
        "8a9a74f40aa0d01fe3ce8219f9a694a69c9e9c2850d1d6b3ff7fff7fbc8a9a75a64ecb25e4060917e0fe9c7eda5210f845887cb2bfe33e7f7fff7fae",
        "b98a567ff3c01b8f17120d4b3e31338480945453bc51dbfc07d355f0c2205efb0748760424f10a972527521e123f36bf633a6d7ff3d25ca02814d92d",
        "112e7528d6722364764185adaf87e7a91e197a52a3d59f3cfd4eb6d08384d1e7d9611bfbc5046ff3d25db10604d92d730e7528d66c6de94c1f493753",
        "e0755c0d0f67adb4ca697885ab5d9ae29f101cf93c1619cafb530cdff37bc0265fb61342c2f862af147dbcec760fff7b44ba554332429af2b9e45141",
        "fd57cfba55433266622c9cf228a0ff4b96f39bf21897120d4b3fa857f142a028fab779d234bc110f0c7435f3ae3329a8123f7c4e05a7387ee91ef357",
        "32b48bec9df3ae3329a8123f7c4e05ab602ea91ef38b753e8bec9dffd9",
    );
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let n = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            n(p[0]) * 16 + n(p[1])
        })
        .collect()
}
/// Require a visible JPX source to select the same detection raster and protected pixels in page/tile renders.
#[test]
fn jpx_detection_raster_and_protected_pixels_are_tile_independent() {
    let pdf =
        hayro::hayro_syntax::Pdf::new(raster_pdf(&jpx_fixture(), 2100, 2100, "/Filter /JPXDecode", "840 0 0 840 4000 0", (5000, 1000))).unwrap();
    crate::barcode::take_rasters();
    let full_cache = hayro::RenderCache::new();
    let full = hayro::render(&pdf.pages()[0], &full_cache, &Default::default(), &Default::default());
    let sizes = crate::barcode::take_rasters();
    assert_eq!(crate::renderer::test_cached_region_count(&full_cache), 1, "JPX barcode must actually be protected");
    let tile = hayro::render(
        &pdf.pages()[0],
        &hayro::RenderCache::new(),
        &Default::default(),
        &hayro::RenderSettings { width: Some(840), height: Some(840), x_offset: 4000.0, y_offset: 160.0, ..Default::default() },
    );
    let tile_sizes = crate::barcode::take_rasters();
    println!("whole_jpx_rasters={sizes:?}; tile_jpx_rasters={tile_sizes:?}");
    assert_eq!(sizes, tile_sizes);
    assert!(!sizes.is_empty());
    for y in 0..840 {
        assert_eq!(
            &full.data_as_u8_slice()[((y + 160) * 5000 + 4000) * 4..((y + 160) * 5000 + 4840) * 4],
            &tile.data_as_u8_slice()[y * 840 * 4..(y + 1) * 840 * 4],
            "tile row {y}"
        );
    }
}

/// Small incremental-cost probe for this review's affected paths; explicitly invoked, never a timing assertion.
#[test]
#[ignore]
fn review_incremental_cost_probe() {
    use std::sync::atomic::AtomicBool;
    for (kind, data, _, _) in [
        {
            let (d, i, q) = qr_fixture();
            ("QR", d, i, q)
        },
        {
            let (d, i, q) = retail_fixture(false, true);
            ("EAN", d, i, q)
        },
        {
            let (d, i, q) = retail_fixture(true, true);
            ("UPC", d, i, q)
        },
    ] {
        let rgba = data.iter().flat_map(|v| [*v, *v, *v, 255]).collect::<Vec<_>>();
        let pdf = hayro::hayro_syntax::Pdf::new(raster_pdf(&data, W, H, "", "700 0 0 1000 0 0", (W, H))).unwrap();
        for observation in 0..3 {
            let start = Instant::now();
            let regions = crate::barcode::detect(W, H, &rgba, None, &AtomicBool::new(false)).unwrap();
            println!("REVIEW_PERF,{kind},detect,{observation},{:.6},{}", start.elapsed().as_secs_f64() * 1000.0, regions.len());
            for tile in [false, true] {
                let cache = hayro::RenderCache::new();
                let settings = hayro::RenderSettings {
                    x_scale: 1.7,
                    y_scale: 1.7,
                    width: tile.then_some(512),
                    height: tile.then_some(512),
                    x_offset: if tile { 100.0 } else { 0.0 },
                    y_offset: if tile { 200.0 } else { 0.0 },
                    ..Default::default()
                };
                for temperature in ["cold", "warm"] {
                    let start = Instant::now();
                    let pix = hayro::render(&pdf.pages()[0], &cache, &Default::default(), &settings);
                    println!(
                        "REVIEW_PERF,{kind},{}-{temperature},{observation},{:.6},{}",
                        if tile { "tile" } else { "full" },
                        start.elapsed().as_secs_f64() * 1000.0,
                        pix.data_as_u8_slice().len()
                    );
                }
            }
        }
    }
    let pdf =
        hayro::hayro_syntax::Pdf::new(raster_pdf(&jpx_fixture(), 2100, 2100, "/Filter /JPXDecode", "840 0 0 840 4000 0", (5000, 1000))).unwrap();
    for observation in 0..3 {
        for tile in [false, true] {
            let cache = hayro::RenderCache::new();
            let settings = hayro::RenderSettings {
                width: tile.then_some(840),
                height: tile.then_some(840),
                x_offset: if tile { 4000.0 } else { 0.0 },
                y_offset: if tile { 160.0 } else { 0.0 },
                ..Default::default()
            };
            for temperature in ["cold", "warm"] {
                let start = Instant::now();
                let pix = hayro::render(&pdf.pages()[0], &cache, &Default::default(), &settings);
                println!(
                    "REVIEW_PERF,JPX,{}-{temperature},{observation},{:.6},{}",
                    if tile { "tile" } else { "full" },
                    start.elapsed().as_secs_f64() * 1000.0,
                    pix.data_as_u8_slice().len()
                );
            }
        }
    }
}

/// Contributor-original Code128-B A with independently specified ink/quiet bounds.
/// Active four-pixel checks consume native calls in all eight scan passes.
/// kind: valid, no barcode, obstructed quiet zone, or too-short bars.
pub(crate) fn call_limit_fixture(tag: u8, kind: u8) -> (Vec<u8>, crate::barcode::Rect, crate::barcode::Rect) {
    use crate::barcode::Rect;
    let mut pixels = vec![255; 1400 * 1400];
    for y in 0..1400 {
        for x in 300..1400 {
            if (x / 4 + y / 4) % 2 == 0 {
                pixels[y * 1400 + x] = 0;
            }
        }
    }
    pixels[0] = tag + 10;
    let ink = Rect { x0: 100, y0: 64, x1: 100 + BITS.len() * 3, y1: 144 };
    let quiet = Rect { x0: 64, y0: 60, x1: ink.x1 + 36, y1: 148 };
    if kind != 1 {
        for y in ink.y0..if kind == 3 { 68 } else { ink.y1 } {
            for (i, b) in BITS.bytes().enumerate() {
                if b == b'1' {
                    pixels[y * 1400 + 100 + i * 3..y * 1400 + 103 + i * 3].fill(0);
                }
            }
        }
    }
    if kind == 2 {
        for y in 64..144 {
            pixels[y * 1400 + 75] = 0;
        }
    }
    (pixels, ink, quiet)
}
/// Build four distinct 1400-square active sources in fixed positions with caller-controlled painting order.
fn call_limit_pdf(order: &[usize]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2800 2800] /Resources << /XObject << /I0 5 0 R /I1 6 0 R /I2 7 0 R /I3 8 0 R >> >> /Contents 4 0 R >>".to_vec(),
    ];
    let content = order.iter().map(|i| format!("q 1400 0 0 1400 {} {} cm /I{i} Do Q\n", i % 2 * 1400, 1400 - i / 2 * 1400)).collect::<String>();
    objects.push(fixture_stream("", content.as_bytes()));
    for i in 0..4 {
        objects.push(fixture_stream(
            "/Type /XObject /Subtype /Image /Width 1400 /Height 1400 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &call_limit_fixture(i, 0).0,
        ));
    }
    objects_pdf(&objects)
}
/// Render the call-limit fixture at 0.4x as a full page or one 560-square device tile.
fn draw_call_limit<'a>(p: &'a hayro::hayro_syntax::Pdf, cache: &hayro::RenderCache<'a>, tile: Option<(usize, usize)>) -> Vec<u8> {
    let (x, y) = tile.unwrap_or((0, 0));
    hayro::render(
        &p.pages()[0],
        cache,
        &Default::default(),
        &hayro::RenderSettings {
            x_scale: 0.4,
            y_scale: 0.4,
            width: tile.map(|_| 560),
            height: tile.map(|_| 560),
            x_offset: x as f32,
            y_offset: y as f32,
            ..Default::default()
        },
    )
    .data_as_u8_slice()
    .to_vec()
}
/// Exercise the real cache/render path at 4096 calls with complete region and independent pixel assertions.
/// Disable only the test clock; require order, warm-cache and whole/tile byte equivalence.
#[test]
fn call_exhaustion_four_image_render_cache_and_tiles() {
    use crate::barcode::{take_native_calls, with_test_decoder, without_test_deadline};
    let p = hayro::hayro_syntax::Pdf::new(call_limit_pdf(&[0, 1, 2, 3])).unwrap();
    let q = hayro::hayro_syntax::Pdf::new(call_limit_pdf(&[3, 2, 1, 0])).unwrap();
    let cache = hayro::RenderCache::new();
    let baseline = with_test_decoder(|_| Some(Ok(Vec::new())), || draw_call_limit(&p, &hayro::RenderCache::new(), None));
    take_native_calls();
    let start = Instant::now();
    let actual = without_test_deadline(|| draw_call_limit(&p, &cache, None));
    let calls = take_native_calls();
    let count = crate::renderer::test_cached_region_count(&cache);
    let different = actual.iter().zip(&baseline).filter(|(a, b)| a != b).count();
    println!(
        "source_pixels=7840000 image_count=4 cached_regions={count} native_calls={calls} cold_ms={:.3} changed_bytes={different}",
        start.elapsed().as_secs_f64() * 1000.
    );
    if let Ok(dir) = std::env::var("PDFCRAFT_CALL_LIMIT_EVIDENCE") {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in [("baseline.rgba", &baseline), ("actual.rgba", &actual)] {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(std::path::Path::new(&dir).join(name)).unwrap();
            f.write_all(data).unwrap();
        }
    }
    assert_eq!(calls, 4096);
    assert_eq!(count, 4, "validated regions must survive later call exhaustion");
    assert!(different > 0);
    // Independent source truth, avoiding float sampling ties at integer coordinates.
    let scale = f64::from(0.4f32);
    let mut checked = 0;
    let mut outside_changes = 0;
    for y in 0..1120 {
        for x in 0..1120 {
            let sx = ((x % 560) as f64 + 0.5) / scale;
            let sy = ((y % 560) as f64 + 0.5) / scale;
            let index = (y * 1120 + x) * 4;
            let (_, _, quiet) = call_limit_bounds();
            if sx >= quiet.x0 as f64 + 1.
                && sx < quiet.x1 as f64 - 1.
                && sy >= quiet.y0 as f64 + 1.
                && sy < quiet.y1 as f64 - 1.
                && sx.fract() > 0.0001
                && sx.fract() < 0.9999
                && sy.fract() > 0.0001
                && sy.fract() < 0.9999
            {
                let source_x = sx.floor() as usize;
                let source_y = sy.floor() as usize;
                let expected = if (100..100 + BITS.len() * 3).contains(&source_x)
                    && (64..144).contains(&source_y)
                    && BITS.as_bytes()[(source_x - 100) / 3] == b'1'
                {
                    0
                } else {
                    255
                };
                assert_eq!(&actual[index..index + 4], &[expected, expected, expected, 255], "x={x} y={y} source={sx},{sy}");
                checked += 1;
            }
            if sx < quiet.x0 as f64 - 2. || sx > quiet.x1 as f64 + 2. || sy < quiet.y0 as f64 - 2. || sy > quiet.y1 as f64 + 2. {
                outside_changes += usize::from(actual[index..index + 4] != baseline[index..index + 4]);
            }
        }
    }
    println!("independent_pixel_checks={checked} outside_changed_pixels={outside_changes}");
    assert_eq!(outside_changes, 0);
    let start = Instant::now();
    assert_eq!(actual, without_test_deadline(|| draw_call_limit(&p, &cache, None)));
    let warm_calls = take_native_calls();
    println!("warm_ms={:.3} warm_native_calls={warm_calls}", start.elapsed().as_secs_f64() * 1000.);
    assert_eq!(warm_calls, 0);
    assert_eq!(actual, without_test_deadline(|| draw_call_limit(&q, &hayro::RenderCache::new(), None)));
    assert!(take_native_calls() <= 4096);
    let mut tiled = vec![0; actual.len()];
    let tile_cache = hayro::RenderCache::new();
    let start = Instant::now();
    for ty in 0..2 {
        for tx in 0..2 {
            let tile = without_test_deadline(|| draw_call_limit(&p, &tile_cache, Some((tx * 560, ty * 560))));
            for y in 0..560 {
                let offset = ((ty * 560 + y) * 1120 + tx * 560) * 4;
                tiled[offset..offset + 560 * 4].copy_from_slice(&tile[y * 560 * 4..(y + 1) * 560 * 4]);
            }
        }
    }
    let tile_calls = take_native_calls();
    println!("tiles_ms={:.3} tile_native_calls={tile_calls}", start.elapsed().as_secs_f64() * 1000.);
    assert!(tile_calls <= 4096);
    assert_eq!(actual, tiled);
}
/// Return fixed source and quiet-zone truth from the generator, independently of decoder points.
fn call_limit_bounds() -> (usize, crate::barcode::Rect, crate::barcode::Rect) {
    use crate::barcode::Rect;
    (1400, Rect { x0: 100, y0: 64, x1: 100 + BITS.len() * 3, y1: 144 }, Rect { x0: 64, y0: 60, x1: 136 + BITS.len() * 3, y1: 148 })
}
/// Record three bounded cold/warm observations using the production clock; never assert a timing speedup.
#[test]
#[ignore = "bounded real-clock observations, run explicitly"]
fn call_exhaustion_real_clock_probe() {
    for observation in 0..3 {
        let p = hayro::hayro_syntax::Pdf::new(call_limit_pdf(&[0, 1, 2, 3])).unwrap();
        let cache = hayro::RenderCache::new();
        crate::barcode::take_native_calls();
        let t = Instant::now();
        draw_call_limit(&p, &cache, None);
        println!(
            "observation={observation} cold_ms={:.3} regions={} calls={}",
            t.elapsed().as_secs_f64() * 1000.,
            crate::renderer::test_cached_region_count(&cache),
            crate::barcode::take_native_calls()
        );
        let t = Instant::now();
        draw_call_limit(&p, &cache, None);
        println!("observation={observation} warm_ms={:.3} calls={}", t.elapsed().as_secs_f64() * 1000., crate::barcode::take_native_calls());
    }
}

/// Draw a bounded whole image or tile while tying cache lifetime to its synthetic PDF.
fn draw_region_cap<'a>(pdf: &'a hayro::hayro_syntax::Pdf, cache: &hayro::RenderCache<'a>, tile: Option<(usize, usize)>) -> Vec<u8> {
    hayro::render(
        &pdf.pages()[0],
        cache,
        &Default::default(),
        &hayro::RenderSettings {
            x_scale: 0.4,
            y_scale: 0.4,
            x_offset: tile.map_or(0., |v| v.0 as f32),
            y_offset: tile.map_or(0., |v| v.1 as f32),
            width: tile.map(|_| 240),
            height: tile.map(|_| 240),
            ..Default::default()
        },
    )
    .data_as_u8_slice()
    .to_vec()
}

/// Paint separate repeated-payload symbols with independently fixed ink/quiet bounds and scanned strokes.
pub(crate) fn region_cap_fixture(count: usize, mixed: bool) -> (Vec<u8>, Vec<crate::barcode::Rect>, Vec<crate::barcode::Rect>) {
    use crate::barcode::Rect;
    let mut data = vec![255; 1200 * 1200];
    let mut inks = Vec::new();
    let mut quiets = Vec::new();
    for i in 0..count {
        let x0 = 80 + i % 4 * 230;
        let y0 = 80 + i / 4 * 170;
        if mixed && i == 0 {
            let (qr, _, _) = qr_fixture();
            for y in 0..100 {
                data[(y0 + y) * 1200 + x0..(y0 + y) * 1200 + x0 + 100].copy_from_slice(&qr[(200 + y) * W + 100..(200 + y) * W + 200]);
            }
            inks.push(Rect { x0, y0, x1: x0 + 100, y1: y0 + 100 });
            quiets.push(Rect { x0: x0 - 16, y0: y0 - 16, x1: x0 + 116, y1: y0 + 116 });
        } else {
            for y in y0..y0 + 64 {
                for (x, bit) in BITS.bytes().enumerate() {
                    if bit == b'1' {
                        data[y * 1200 + x0 + x * 3..y * 1200 + x0 + x * 3 + 3].fill(0);
                    }
                }
            }
            inks.push(Rect { x0, y0, x1: x0 + BITS.len() * 3, y1: y0 + 64 });
            quiets.push(Rect { x0: x0 - 36, y0: y0 - 4, x1: x0 + BITS.len() * 3 + 36, y1: y0 + 68 });
        }
    }
    for y in 1060..1100 {
        for x in (850..1100).step_by(7) {
            data[y * 1200 + x] = 0;
        }
    }
    (data, inks, quiets)
}

/// Characterize the image-wide cap: duplicates do not proliferate; a real seventeenth returns failure.
#[test]
fn region_cap_detector_keeps_sixteen_and_declines_seventeen() {
    use std::sync::atomic::AtomicBool;
    for mixed in [false, true] {
        for count in [16, 17] {
            let (data, inks, quiets) = region_cap_fixture(count, mixed);
            let rgba = data.iter().flat_map(|v| [*v, *v, *v, 255]).collect::<Vec<_>>();
            let result = crate::barcode::detect(1200, 1200, &rgba, None, &AtomicBool::new(false));
            println!("region_cap mixed={mixed} count={count} result={:?}", result.as_ref().map(|r| r.len()));
            if count == 17 {
                assert_eq!(result.unwrap_err(), "region budget; discard all");
            } else {
                let regions = result.unwrap();
                assert_eq!(regions.len(), 16);
                for (i, (&ink, &quiet)) in inks.iter().zip(&quiets).enumerate() {
                    let found = regions.iter().find(|r| r.ink == ink).unwrap();
                    assert!(found.quiet.contains(quiet));
                    if !mixed || i != 0 {
                        assert_eq!(found.format, "CODE_128");
                        assert_eq!(found.payload, "A");
                    }
                }
            }
        }
    }
}

/// Retain the documented all-or-nothing smoothing fallback across source cache reuse and tile draws.
#[test]
fn region_cap_render_fallback_cache_and_tiles() {
    use crate::barcode::{take_native_calls, with_test_decoder};
    for mixed in [false, true] {
        for count in [16, 17] {
            let (data, _, quiets) = region_cap_fixture(count, mixed);
            let pdf = hayro::hayro_syntax::Pdf::new(raster_pdf(&data, 1200, 1200, "", "1200 0 0 1200 0 0", (1200, 1200))).unwrap();
            let fallback = with_test_decoder(|_| Some(Ok(Vec::new())), || draw_region_cap(&pdf, &hayro::RenderCache::new(), None));
            let cache = hayro::RenderCache::new();
            take_native_calls();
            let actual = draw_region_cap(&pdf, &cache, None);
            let cold_calls = take_native_calls();
            assert_eq!(crate::renderer::test_cached_region_count(&cache), if count == 16 { 16 } else { 0 });
            assert_eq!(draw_region_cap(&pdf, &cache, None), actual);
            assert_eq!(take_native_calls(), 0, "positive and negative cache hits must not rescan");
            let different = actual.chunks_exact(4).zip(fallback.chunks_exact(4)).filter(|(a, b)| a != b).count();
            if count == 17 {
                assert_eq!(different, 0, "overflow must preserve ordinary smoothing for the entire source");
            } else {
                assert!(different > 0);
                for y in 0..480 {
                    for x in 0..480 {
                        let sx = ((x as f64 + 0.5) / f64::from(0.4_f32)).floor() as usize;
                        let sy = ((y as f64 + 0.5) / f64::from(0.4_f32)).floor() as usize;
                        let at = (y * 480 + x) * 4;
                        if quiets.iter().any(|q| sx > q.x0 && sx + 1 < q.x1 && sy > q.y0 && sy + 1 < q.y1) {
                            assert_eq!(actual[at], data[sy * 1200 + sx], "protected source sample {x},{y}");
                        } else if !quiets.iter().any(|q| sx + 2 >= q.x0 && sx <= q.x1 + 2 && sy + 2 >= q.y0 && sy <= q.y1 + 2) {
                            assert_eq!(actual[at..at + 4], fallback[at..at + 4], "outside region {x},{y}");
                        }
                    }
                }
            }
            let mut stitched = vec![0; actual.len()];
            let tile_cache = hayro::RenderCache::new();
            for ty in 0..2 {
                for tx in 0..2 {
                    let tile = draw_region_cap(&pdf, &tile_cache, Some((tx * 240, ty * 240)));
                    for y in 0..240 {
                        let to = ((ty * 240 + y) * 480 + tx * 240) * 4;
                        stitched[to..to + 240 * 4].copy_from_slice(&tile[y * 240 * 4..(y + 1) * 240 * 4]);
                    }
                }
            }
            assert_eq!(stitched, actual);
            println!(
                "region_pixels mixed={mixed} count={count} changed_pixels={different} cold_calls={cold_calls} warm_calls=0 tile_different_bytes=0"
            );
        }
    }
}
