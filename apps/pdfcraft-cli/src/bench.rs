//! `pdfcraft-cli bench`: Mobile performance baselines (open time, tile render latency p50/p95, peak RSS).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, Tile};
use serde_json::json;

use crate::{CliError, OptionSpec, flag, positional, validate_options};

pub const BENCH_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--json", takes_value: true },
    OptionSpec { name: "--tile-sizes", takes_value: true },
    OptionSpec { name: "--scale", takes_value: true },
    OptionSpec { name: "--synthetic", takes_value: false },
];

pub struct TileMetrics {
    pub count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

pub struct DocumentBenchmark {
    pub name: String,
    pub path: String,
    pub pages: usize,
    pub file_size_bytes: usize,
    pub open_ms: f64,
    pub tiles: std::collections::BTreeMap<u32, TileMetrics>,
}

pub fn peak_rss_mb() -> f64 {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        if let Ok(output) = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
            && let Ok(kb) = text.trim().parse::<f64>()
        {
            return kb / 1024.0;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmHWM:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if let Some(kb_str) = parts.get(1)
                        && let Ok(kb) = kb_str.parse::<f64>()
                    {
                        return kb / 1024.0;
                    }
                }
            }
        }
    }
    0.0
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 * pct).ceil() as usize).saturating_sub(1);
    let clamped = idx.min(sorted.len().saturating_sub(1));
    sorted.get(clamped).copied().unwrap_or(0.0)
}

/// A minimal synthetic PDF with `n` pages showing text.
pub fn synthetic_pdf(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 595 842] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 14 Tf 50 800 Td (PdfCraft Mobile Baseline Document - Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

pub fn bench(args: &[String]) -> Result<(), CliError> {
    validate_options("bench", args, BENCH_OPTIONS)?;
    let tile_sizes: Vec<u32> = flag(args, "--tile-sizes").unwrap_or("256,512,1024").split(',').filter_map(|s| s.trim().parse::<u32>().ok()).collect();
    let scale: f32 = flag(args, "--scale").unwrap_or("2.0").parse().map_err(|_| "invalid --scale")?;

    let positional_files = positional(args, BENCH_OPTIONS);
    let mut documents_to_bench: Vec<(String, String, Vec<u8>)> = Vec::new();

    if positional_files.is_empty() || args.iter().any(|a| a == "--synthetic") {
        documents_to_bench.push(("synthetic-text-10p".into(), "synthetic://text-10p".into(), synthetic_pdf(10)));
        documents_to_bench.push(("synthetic-text-50p".into(), "synthetic://text-50p".into(), synthetic_pdf(50)));
    }

    for path_str in positional_files {
        let p = Path::new(path_str);
        if p.is_file()
            && let Ok(bytes) = std::fs::read(p)
        {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path_str.to_string());
            documents_to_bench.push((name, path_str.to_string(), bytes));
        }
    }

    if documents_to_bench.is_empty() {
        return Err("bench: no documents provided or generated".into());
    }

    let mut doc_metrics = Vec::new();

    for (name, path, bytes) in documents_to_bench {
        let file_size_bytes = bytes.len();
        let bytes_arc = Arc::new(bytes);

        let t_open = Instant::now();
        let mut renderer = PageRenderer::new(bytes_arc, RenderConfig::default());
        let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
        let pages = renderer.page_count();

        let mut tiles_map = std::collections::BTreeMap::new();

        let (page_w_pt, page_h_pt) = (595.0f32, 842.0f32);
        let page_w_px = (page_w_pt * scale).ceil() as u32;
        let page_h_px = (page_h_pt * scale).ceil() as u32;

        for &tile_size in &tile_sizes {
            if tile_size == 0 {
                continue;
            }
            let mut tile_times = Vec::new();
            let mut y = 0;
            while y < page_h_px {
                let mut x = 0;
                while x < page_w_px {
                    let w = tile_size.min(page_w_px.saturating_sub(x));
                    let h = tile_size.min(page_h_px.saturating_sub(y));
                    let tile = Tile { x, y, w, h };
                    let t_tile = Instant::now();
                    let _ = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: Some(tile), scale, tag: 0, preview: false });
                    tile_times.push(t_tile.elapsed().as_secs_f64() * 1000.0);
                    x = x.saturating_add(tile_size);
                }
                y = y.saturating_add(tile_size);
            }

            tile_times.sort_by(|a, b| a.total_cmp(b));
            let p50_ms = percentile(&tile_times, 0.50);
            let p95_ms = percentile(&tile_times, 0.95);
            tiles_map.insert(tile_size, TileMetrics { count: tile_times.len(), p50_ms, p95_ms });
        }

        doc_metrics.push(DocumentBenchmark { name, path, pages, file_size_bytes, open_ms, tiles: tiles_map });
    }

    let peak_rss = peak_rss_mb();

    println!("{:<24} {:>8} {:>10} {:>12} {:>12}", "Document", "Pages", "Open (ms)", "Tile 512 p50", "Tile 512 p95");
    println!("{:-<24} {:-<8} {:-<10} {:-<12} {:-<12}", "", "", "", "", "");
    for d in &doc_metrics {
        let (t512_p50, t512_p95) =
            d.tiles.get(&512).map(|t| (format!("{:.2} ms", t.p50_ms), format!("{:.2} ms", t.p95_ms))).unwrap_or(("-".into(), "-".into()));
        println!("{:<24} {:>8} {:>9.2}ms {:>12} {:>12}", d.name, d.pages, d.open_ms, t512_p50, t512_p95);
    }
    println!("\nPeak RSS: {:.2} MB", peak_rss);

    if let Some(json_out) = flag(args, "--json") {
        let docs_json: Vec<serde_json::Value> = doc_metrics
            .iter()
            .map(|d| {
                let tiles_json: serde_json::Map<String, serde_json::Value> = d
                    .tiles
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            json!({
                                "count": v.count,
                                "p50_ms": v.p50_ms,
                                "p95_ms": v.p95_ms,
                            }),
                        )
                    })
                    .collect();

                json!({
                    "name": d.name,
                    "path": d.path,
                    "pages": d.pages,
                    "file_size_bytes": d.file_size_bytes,
                    "open_ms": d.open_ms,
                    "tiles": tiles_json,
                })
            })
            .collect();

        let report_json = json!({
            "target": std::env::consts::ARCH,
            "peak_rss_mb": peak_rss,
            "documents": docs_json,
        });

        let json_str = serde_json::to_string_pretty(&report_json).map_err(|e| format!("json error: {e}"))?;
        if let Some(parent) = Path::new(json_out).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(json_out, json_str.as_bytes()).map_err(|e| format!("cannot write {json_out}: {e}"))?;
        println!("Baseline written to {json_out}");
    }

    Ok(())
}
