//! Synthetic multi-image regression; positions and barcode samples are independent of detection.
use crate as hayro;
use std::time::Instant;
const W: usize = 700;
const H: usize = 1000;
const BITS: &str = "1101001000010100011000100010110001100011101011";
fn pdf(order: &[usize]) -> Vec<u8> {
    pdf_sized(order, W, H)
}
fn pdf_sized(order: &[usize], width: usize, height: usize) -> Vec<u8> {
    pdf_layered(order, width, height, false)
}
fn pdf_layered(order: &[usize], width: usize, height: usize, layered: bool) -> Vec<u8> {
    pdf_content(order, width, height, layered, false)
}
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
#[test]
fn image_order_cache_history_eviction_and_tiles_agree() {
    for count in [7, 11, 12, 13] {
        check(count);
    }
}
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
