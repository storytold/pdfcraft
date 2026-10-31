//! Headless screenshot of the real PdfCraft shell (egui_kittest + wgpu, no window needed).
//!
//! ```text
//! cargo run -p pdfcraft-ui-egui --example shot -- out.png [file.pdf] [--size 1440x900] [--scale 2] [--page 3 --panel pages …]
//! ```
//! `--width N` downscales the image to N pixels wide (README screenshots). Any other
//! `--key value` pair is passed to `PdfCraftApp::set_option`
//! (the same verbs as the desktop app's command-line flags and the future control channel).
//! `--ocr-verify on` ignores any file and instead builds a synthetic scanned page, recognizes
//! it with the real engines (models required: `cargo xtask models`) and opens the OCR Verify
//! screen on the result, as a reviewer sees it.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pdfcraft_engine::ocr::{MergeStrategy, OcrSettings};
use pdfcraft_engine::{Session, export};
use pdfcraft_ui_egui::PdfCraftApp;

/// Build a one-page PDF that is only a picture of a sentence (as a scanner would produce),
/// recognize it with the engines named in `settings` and open the OCR Verify screen on the
/// result — the same path the review tests use, with a real recognition behind it. The toolbar
/// dropdowns are seeded to the ensemble the shot ran.
fn load_ocr_verify(app: &mut PdfCraftApp) -> Result<(), String> {
    let mut s = Session::new();
    let text = s
        .create_from_text(
            "t",
            "Scanned pages are pictures of words.\nThe Verify screen reads them back before anything is applied.\nReview the uncertain words, then Accept.",
        )
        .map_err(|e| e.to_string())?;
    let id = s.open("text.pdf", None, text, None).map_err(|e| e.to_string())?;
    let png = export::Exporter::new(s.get(id).ok_or("the document is gone")?).png(0, 150.0).map_err(|e| e.to_string())?;
    // Keep the text block of the rendered page, so the scan is a short note-sized picture:
    // the whole page and the confidence legend fit in the shot's source-page pane.
    let mut page = image::load_from_memory(&png).map_err(|e| e.to_string())?;
    let mut scan_png = std::io::Cursor::new(Vec::new());
    page.crop(0, 0, page.width(), (page.height() as f32 * 0.34) as u32)
        .write_to(&mut scan_png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let scan = s.create_from_images(&[("scan.png".into(), scan_png.into_inner())]).map_err(|e| e.to_string())?.to_vec();
    let settings =
        OcrSettings { engine: pdfcraft_engine::ocr::EngineChoice::Auto, strategy: MergeStrategy::ConfidenceWeighted, ..OcrSettings::default() };
    app.ocr_draft.engine = settings.engine;
    app.ocr_draft.strategy = settings.strategy;
    app.open_bytes("scan.pdf", None, scan).map_err(|e| e.to_string())?;
    app.run_inline = true;
    if !app.execute("ocr.correct") {
        return Err("ocr.correct did not run".into());
    }
    let Some((_, id)) = app.active_ids() else { return Err("no open document".into()) };
    let job = app.session.ocr_job(id, &[], settings.clone()).ok_or("the OCR job could not be captured")?;
    let recognizers = pdfcraft_engine::ocr::recognizers(&settings)?;
    let guard = job.guard.clone();
    let pages = job.run(&recognizers, |_, _| true);
    app.ocr_verify.load(id, guard, pages);
    Ok(())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: shot <out.png> [file.pdf] [--option value …]")?;
    let (mut file, mut opts) = (None, Vec::new());
    let (mut size, mut scale) = (egui::vec2(1440.0, 900.0), 2.0f32);
    let mut width: Option<u32> = None;
    let mut ocr_verify = false;
    while let Some(a) = args.next() {
        match a.strip_prefix("--") {
            Some("size") => {
                let v = args.next().unwrap_or_default();
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                size = egui::vec2(w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            Some("scale") => scale = args.next().and_then(|v| v.parse().ok()).ok_or("bad --scale")?,
            Some("width") => width = Some(args.next().and_then(|v| v.parse().ok()).ok_or("bad --width")?),
            Some("ocr-verify") => ocr_verify = args.next().is_some_and(|v| v == "on"),
            Some(k) => opts.push((k.to_string(), args.next().unwrap_or_default())),
            None => file = Some(a),
        }
    }
    let mut harness = Harness::builder().with_size(size).with_pixels_per_point(scale).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        if ocr_verify {
            if let Err(e) = load_ocr_verify(&mut app) {
                eprintln!("shot: --ocr-verify: {e}");
            }
        } else if let Some(f) = &file {
            let bytes = std::fs::read(f).unwrap_or_default();
            let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Err(e) = app.open_bytes(&name, Some(f.clone()), bytes) {
                eprintln!("shot: {f}: {e}");
            }
        }
        for (k, v) in &opts {
            if let Err(e) = app.set_option(k, v) {
                eprintln!("shot: --{k} {v}: {e}");
            }
        }
        app
    });
    // Let fonts install, layout settle and background renders arrive.
    let start = Instant::now();
    harness.run_steps(4);
    while start.elapsed() < Duration::from_secs(20) {
        harness.run_steps(2);
        if !harness.state().render_pending() {
            harness.run_steps(3);
            if !harness.state().render_pending() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut image = harness.render()?;
    if let Some(w) = width.filter(|w| *w < image.width()) {
        let h = (image.height() as f64 * w as f64 / image.width() as f64).round() as u32;
        image = image::imageops::resize(&image, w, h, image::imageops::FilterType::Lanczos3);
    }
    image.save(&out).map_err(|e| e.to_string())?;
    eprintln!("shot: wrote {out} ({}×{})", image.width(), image.height());
    Ok(())
}
