//! Optimize a PDF from the command line with the optimizer's presets, or to a size:
//!
//! ```text
//! cargo run --release -p pdfcraft-optimize --example compress -- detect  in.pdf
//! cargo run --release -p pdfcraft-optimize --example compress -- preset  in.pdf out.pdf presentation
//! cargo run --release -p pdfcraft-optimize --example compress -- target  in.pdf out.pdf presentation 20
//! ```
//!
//! `detect` lists the pages made of many image tiles; `preset` applies a preset; `target` finds
//! the sharpest settings whose result is at most the given number of megabytes.

use std::sync::Arc;
use std::time::Instant;

use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_optimize::{Preset, Settings, Stage, detect_tiled_pages, optimize_to_target, optimize_with_progress};

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first().map(String::as_str) else { fail("usage: compress detect|preset|target in.pdf [out.pdf preset [mb]]") };
    let input = args.get(1).unwrap_or_else(|| fail("missing input"));
    let data = std::fs::read(input).unwrap_or_else(|e| fail(&format!("{input}: {e}")));
    let original = data.len();
    let doc = Document::open(Arc::new(data)).unwrap_or_else(|e| fail(&format!("not a readable PDF: {e}")));
    let started = Instant::now();
    if mode == "detect" {
        let pages = detect_tiled_pages(&doc, Settings::default().flatten_tiles.min_images);
        println!("{} tiled page(s):", pages.len());
        for p in &pages {
            println!(
                "  page {:>3}: {:>4} images, {:>4} tiles, {:>4} touching pairs, coverage {:.2}{}",
                p.index + 1,
                p.images,
                p.tiles,
                p.touching,
                p.coverage,
                p.skip.map(|s| format!("  [left as is: {}]", s.reason())).unwrap_or_default()
            );
        }
        println!("scanned in {:.1}s", started.elapsed().as_secs_f64());
        return;
    }
    let out = args.get(2).unwrap_or_else(|| fail("missing output"));
    let preset = args.get(3).map(String::as_str).unwrap_or("presentation");
    let preset = Preset::from_name(preset).unwrap_or_else(|| fail(&format!("unknown preset {preset}")));
    let mut progress = |stage: Stage| {
        match stage {
            Stage::Flatten { done, total } => eprint!("\rflattening tiled page {done}/{total}      "),
            Stage::Images { done, total } if done % 25 == 0 || done == total => eprint!("\rimage {done}/{total}      "),
            Stage::Trial { n, of } => eprintln!("\ntrial {n} (of at most {of})"),
            _ => {}
        }
        true
    };
    let (bytes, report) = match mode {
        "preset" => {
            let settings = preset.settings();
            let mut doc = doc;
            let report = optimize_with_progress(&mut doc, &settings, &mut progress).unwrap_or_else(|e| fail(&format!("{e}")));
            let opts = SaveOptions { object_streams: settings.effective_object_streams(), ..SaveOptions::default() };
            (write_full(&doc, &opts).unwrap_or_else(|e| fail(&format!("{e}"))), report)
        }
        "target" => {
            let target: f64 = args.get(4).and_then(|t| t.parse().ok()).unwrap_or_else(|| fail("missing target size in MB"));
            let target_bytes = (target * 1024.0 * 1024.0) as u64;
            let outcome = optimize_to_target(&doc, &preset.settings(), target_bytes, &mut progress)
                .unwrap_or_else(|e| fail(&format!("{e}")));
            eprintln!();
            for t in &outcome.trials {
                println!("  tried longest side {:>4} px, JPEG quality {:>2}: {:.2} MB", t.max_dimension, t.quality, mb(t.bytes));
            }
            println!("chose longest side {:?} px: fits {target} MB: {}", outcome.settings.max_image_dimension, outcome.fits);
            let opts = SaveOptions { object_streams: outcome.settings.effective_object_streams(), ..SaveOptions::default() };
            (write_full(&outcome.doc, &opts).unwrap_or_else(|e| fail(&format!("{e}"))), outcome.report)
        }
        other => fail(&format!("unknown mode {other}")),
    };
    std::fs::write(out, &bytes).unwrap_or_else(|e| fail(&format!("{out}: {e}")));
    eprintln!();
    println!("{report:#?}");
    println!("{:.2} MB -> {:.2} MB in {:.1}s", mb(original), mb(bytes.len()), started.elapsed().as_secs_f64());
}

