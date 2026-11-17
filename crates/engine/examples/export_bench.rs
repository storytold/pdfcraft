//! Times Export ▸ Image over every page of a PDF, for before and after comparisons.
//!
//! Usage: `export_bench FILE [DPI] [png|jpeg|tiff] [RUNS]` (defaults: 150 dpi, PNG, 7 runs). Each
//! run exports every page through [`Exporter::images`] from a fresh exporter, as Export ▸ Image
//! does, and prints its wall time; the summary gives the median and the minimum, the total size
//! and an FNV-1a hash of every file in page order. Equal hashes mean equal output. Build with
//! `--release`; timings are diagnostic, never a CI deadline.

use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use pdfcraft_engine::Session;
use pdfcraft_engine::export::{Exporter, ImageFormat};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: export_bench FILE [DPI] [png|jpeg|tiff] [RUNS]")?;
    let dpi: f64 = args.next().map_or(Ok(150.0), |s| s.parse())?;
    let format = match args.next().as_deref() {
        None | Some("png") => ImageFormat::Png,
        Some("jpeg" | "jpg") => ImageFormat::Jpeg { quality: 85 },
        Some("tiff" | "tif") => ImageFormat::Tiff,
        Some(other) => return Err(format!("unknown format {other:?}").into()),
    };
    let runs: usize = args.next().map_or(Ok(7), |s| s.parse())?;
    let mut session = Session::new();
    let id = session.open("bench.pdf", None, Arc::new(std::fs::read(&path)?), None)?;
    let src = session.get(id).ok_or("the document did not open")?.export_source();
    let pages: Vec<usize> = (0..src.pages).collect();
    let mut times = Vec::new();
    let mut summary = None;
    for run in 0..runs {
        let start = Instant::now();
        let mut exporter = Exporter::from_source(src.clone());
        let (mut hash, mut bytes) = (FNV_OFFSET, 0usize);
        exporter.images(&pages, dpi, format, |_, file| {
            bytes += file.len();
            for b in file {
                hash = (hash ^ u64::from(b)).wrapping_mul(FNV_PRIME);
            }
            Ok(())
        })?;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        println!("run {}: {ms:.1} ms", run + 1);
        times.push(ms);
        summary = Some((bytes, hash));
    }
    times.sort_by(f64::total_cmp);
    if let (Some(median), Some(min), Some((bytes, hash))) = (times.get(times.len() / 2), times.first(), summary) {
        println!(
            "{} pages, {} at {dpi} dpi: median {median:.1} ms, min {min:.1} ms ({runs} runs), {bytes} bytes, hash {hash:016x}",
            pages.len(),
            format.label()
        );
    }
    Ok(())
}
