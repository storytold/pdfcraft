//! Times and hashes the renders the desktop canvas asks for on page 1 of a PDF, for before and after
//! comparisons (storytold/pdfcraft#623, item 8).
//!
//! Each round queues one scenario's requests on a render pool with the engine's worker count and
//! prints the time to the first answer, the wall time, the slowest request, the text glyph count,
//! the warning count, the error count, and an FNV-1a hash of every answer's size, pixels and text
//! layer in request order. Equal hashes mean equal output. Peak memory is not measured here; on
//! macOS wrap the command in `/usr/bin/time -l`, and subtract the `load` scenario to get the
//! render's share.
//!
//! Usage: `page_timing FILE SCENARIO [ROUNDS] [THREADS]`. Scenarios:
//! - `load`: open the file and start the pool with no request.
//! - `fit`: page 1 at 2800 device pixels wide.
//! - `backdrop`: page 1 at 2048 pixels on its longest side, the canvas's low-resolution backdrop.
//! - `thumb`: a 240 × 320 thumbnail at density 2.
//! - `tiles100`, `tiles400`: four 1024 px tiles at 100% and 400% on a 2× display.
//! - `tiny`: one 16 px tile, which shows the fixed cost of a request.
//! - `text`: the text layer of page 1.
//! - `sweep`: whole pages and tiles at six scales, the broadest identity check.

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_render::{PageText, RenderConfig, RenderPool, RenderRequest, RenderedPage, RequestKind, Tile};

/// The canvas's tile side in device pixels.
const TILE: u32 = 1024;
const BACKDROP_SIDE: f32 = 2048.0;
const FIT_WIDTH: f32 = 2800.0;
const THUMB_W: f32 = 240.0;
const THUMB_H: f32 = 320.0;
const THUMB_DENSITY: f32 = 2.0;
/// The tag bit that marks a thumbnail request, as the canvas sets it.
const THUMB_TAG: u64 = 1 << 63;
const TEXT_TAG: u64 = 1 << 62;
/// Device pixels per point at 100% on a 2× display.
const SCREEN_SCALE: f32 = 96.0 / 72.0 * 2.0;
const TIMEOUT: Duration = Duration::from_secs(600);
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(scenario)) = (args.get(1), args.get(2)) else {
        return Err("usage: page_timing FILE SCENARIO [ROUNDS] [THREADS]".into());
    };
    let rounds: usize = match args.get(3) {
        Some(s) => s.parse()?,
        None => 1,
    };
    let threads: usize = match args.get(4) {
        Some(s) => s.parse::<usize>()?.max(1),
        None => std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8) - 1,
    };

    let bytes = Arc::new(std::fs::read(path)?);
    let (width, height) = {
        let pdf = hayro_syntax::Pdf::new_with_password(bytes.clone(), "").map_err(|e| format!("{e:?}"))?;
        let pages = pdf.pages();
        pages.first().ok_or("the document has no pages")?.render_dimensions()
    };
    let requests = scenario_requests(scenario, width, height)?;

    let pool = RenderPool::new(bytes, threads, RenderConfig::default());
    for round in 0..rounds {
        let (mut results, first, wall) = run(&pool, &requests)?;
        results.sort_by_key(|r| (r.request.page, r.request.scale.to_bits(), r.request.tile.map(|t| (t.x, t.y, t.w, t.h))));
        let slowest = results.iter().map(|r| r.millis).max().unwrap_or(0);
        let glyphs: usize = results.iter().filter_map(|r| r.text.as_ref()).map(|t| t.glyphs.len()).sum();
        let warnings: usize = results.iter().map(|r| r.warnings.len()).sum();
        let errors: Vec<&str> = results.iter().filter_map(|r| r.error.as_deref()).collect();
        let mut hash = FNV_OFFSET;
        for r in &results {
            hash = fnv1a(hash, &r.width.to_le_bytes());
            hash = fnv1a(hash, &r.height.to_le_bytes());
            hash = fnv1a(hash, &r.rgba[..]);
            if let Some(text) = &r.text {
                hash = hash_text(hash, text);
            }
        }
        println!(
            "round={round} scenario={scenario} page_pt={width}x{height} threads={threads} requests={} first_ms={} wall_ms={} slowest_ms={slowest} glyphs={glyphs} warnings={warnings} errors={} hash={hash:016x}",
            requests.len(),
            first.as_millis(),
            wall.as_millis(),
            errors.len()
        );
        if let Some(first_error) = errors.first() {
            println!("first_error={first_error}");
        }
        if let Some(first_warning) = results.iter().flat_map(|r| r.warnings.iter()).next() {
            println!("first_warning={first_warning:?}");
        }
    }
    Ok(())
}

/// The requests of one scenario for a page of `width` × `height` points.
fn scenario_requests(scenario: &str, width: f32, height: f32) -> Result<Vec<RenderRequest>, String> {
    let page = |scale: f32, tile: Option<Tile>, preview: bool| RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile,
        scale,
        tag: scale_tag(scale),
        preview,
    };
    let tiles = |scale: f32| -> Vec<RenderRequest> {
        [(0, 0), (TILE, 0), (0, TILE), (TILE, TILE)].into_iter().map(|(x, y)| page(scale, Some(Tile { x, y, w: TILE, h: TILE }), false)).collect()
    };
    let requests = match scenario {
        "load" => Vec::new(),
        "fit" => vec![page(FIT_WIDTH / width, None, false)],
        "backdrop" => vec![page(BACKDROP_SIDE / width.max(height), None, true)],
        "thumb" => {
            let scale = (THUMB_W / width).min(THUMB_H / height) * THUMB_DENSITY;
            vec![RenderRequest { tag: THUMB_TAG | u64::from(scale.to_bits()), ..page(scale, None, true) }]
        }
        "tiles100" => tiles(SCREEN_SCALE),
        "tiles400" => tiles(4.0 * SCREEN_SCALE),
        "tiny" => vec![page(SCREEN_SCALE, Some(Tile { x: 0, y: 0, w: 16, h: 16 }), false)],
        "text" => vec![RenderRequest { kind: RequestKind::Text, tag: TEXT_TAG, ..page(1.0, None, false) }],
        "sweep" => {
            let mut out = Vec::new();
            for scale in [0.37_f32, 1.0, 1.7, 2.666_667, 5.5, 10.666_667] {
                out.push(page(scale, None, false));
                for side in [333_u32, 1024, 2048] {
                    for (x, y) in [(0_u32, 0_u32), (811, 507), (1500, 900)] {
                        out.push(page(scale, Some(Tile { x, y, w: side, h: side }), false));
                    }
                }
            }
            out
        }
        other => return Err(format!("unknown scenario `{other}`")),
    };
    Ok(requests)
}

/// Queues `requests` as the canvas does and waits for every answer. Returns the answers, the time
/// to the first one, and the wall time.
fn run(pool: &RenderPool, requests: &[RenderRequest]) -> Result<(Vec<RenderedPage>, Duration, Duration), String> {
    let start = Instant::now();
    pool.set_queue(requests.to_vec());
    let mut results = Vec::with_capacity(requests.len());
    let mut first = None;
    while results.len() < requests.len() {
        match pool.try_recv() {
            Some(page) => {
                first.get_or_insert_with(|| start.elapsed());
                results.push(page);
            }
            None if start.elapsed() > TIMEOUT => {
                return Err(format!("{} of {} answers after {TIMEOUT:?}", results.len(), requests.len()));
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Ok((results, first.unwrap_or_default(), start.elapsed()))
}

/// The canvas's tag for a scale: equal tags mean the same scale to 1/65536.
fn scale_tag(scale: f32) -> u64 {
    (f64::from(scale) * 65536.0).round() as u64
}

/// Folds `bytes` into an FNV-1a hash.
fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Folds a text layer into `hash`: each glyph's text and boxes, then the line and space structure.
fn hash_text(mut hash: u64, text: &PageText) -> u64 {
    for glyph in &text.glyphs {
        hash = fnv1a(hash, glyph.text.as_bytes());
        for value in glyph.rect.iter().chain(glyph.quad.iter().flatten()).chain(glyph.direction.iter()) {
            hash = fnv1a(hash, &value.to_bits().to_le_bytes());
        }
    }
    for line in &text.line_of {
        hash = fnv1a(hash, &line.to_le_bytes());
    }
    for &space in &text.space_before {
        hash = fnv1a(hash, &[u8::from(space)]);
    }
    hash
}
