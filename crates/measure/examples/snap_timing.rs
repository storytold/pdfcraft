//! Times extracting a page's snapping geometry and snapping at points on it.
//!
//! Usage: `snap_timing FILE PAGE` (PAGE is 1-based).

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(page)) = (args.get(1), args.get(2)) else {
        return Err("usage: snap_timing FILE PAGE".into());
    };
    let page: usize = page.parse::<usize>()?.checked_sub(1).ok_or("pages are numbered from 1")?;
    let bytes = std::sync::Arc::new(std::fs::read(path)?);
    let doc = pdfcraft_cos::Document::open(bytes).map_err(|e| format!("{e:?}"))?;
    let start = Instant::now();
    let g = pdfcraft_measure::snap::geometry(&doc, page)?;
    let extracted = start.elapsed();
    println!("extracted {} segments in {} ms: truncated {} unreadable {}", g.segment_count(), extracted.as_millis(), g.truncated, g.unreadable);
    // Snaps at points spread over the drawing, with a 6-unit tolerance (a few pixels on screen).
    let points: Vec<[f64; 2]> =
        g.segments().step_by((g.segment_count() / 2000).max(1)).map(|[a, b]| [(a[0] + b[0]) / 2.0 + 0.7, (a[1] + b[1]) / 2.0 - 0.4]).collect();
    let start = Instant::now();
    let mut hits = 0;
    for &p in &points {
        if g.snap(p, 6.0, pdfcraft_measure::snap::SnapOptions::default())?.is_some() {
            hits += 1;
        }
    }
    let per = start.elapsed().as_secs_f64() * 1e6 / points.len().max(1) as f64;
    println!("{} snaps, {hits} hits, {per:.0} us per snap", points.len());
    Ok(())
}
