//! Synthetic repeated-image benchmark for the derived-image cache opportunity in #2436.
//! Usage: image_cache_bench [source-side=5000] [copies=9] [rounds=3] [interpolate=true].
//! Writes its contributor-original fixture under target/image-cache-bench/ for headless/oracle
//! checks. Run each measurement in a fresh process and collect its peak working set externally.

use std::error::Error;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use flate2::{Compression, write::ZlibEncoder};
use hayro::hayro_interpret::InterpreterSettings;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> =
        std::env::args_os().map(|arg| arg.into_string().map_err(|_| "arguments must be valid Unicode")).collect::<Result<_, _>>()?;
    let side: usize = args.get(1).map_or(Ok(5000), |s| s.parse())?;
    let copies: usize = args.get(2).map_or(Ok(9), |s| s.parse())?;
    let rounds: usize = args.get(3).map_or(Ok(3), |s| s.parse())?;
    let interpolate: bool = args.get(4).map_or(Ok(true), |s| s.parse())?;
    if !(32..=6000).contains(&side) || !(1..=16).contains(&copies) || !(1..=20).contains(&rounds) {
        return Err("source-side must be 32..=6000, copies 1..=16, rounds 1..=20".into());
    }
    // A bounded, deterministic RGB image, generated row-by-row without a full source buffer.
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    let mut row = Vec::new();
    row.try_reserve_exact(side.checked_mul(3).ok_or("row size overflow")?)?;
    for y in 0..side {
        row.clear();
        for x in 0..side {
            row.extend([(x % 251) as u8, (y % 241) as u8, ((x / 17 + y / 13) % 239) as u8]);
        }
        encoder.write_all(&row)?;
    }
    let image = encoder.finish()?;
    let mut content = String::new();
    for n in 0..copies {
        content.push_str(&format!("q 240 0 0 240 {} {} cm /Im Do Q\n", (n % 4) * 250, (n / 4) * 250));
    }
    let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1000 1000] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >> endobj\n".to_vec();
    write!(pdf, "4 0 obj << /Length {} >> stream\n{content}endstream endobj\n", content.len())?;
    writeln!(
        pdf,
        "5 0 obj << /Type /XObject /Subtype /Image /Width {side} /Height {side} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Interpolate {interpolate} /Filter /FlateDecode /Length {} >> stream",
        image.len()
    )?;
    pdf.extend_from_slice(&image);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    std::fs::create_dir_all("target/image-cache-bench")?;
    std::fs::write("target/image-cache-bench/repeated.pdf", &pdf)?;
    let pdf = hayro_syntax::Pdf::new(Arc::new(pdf)).map_err(|e| format!("{e:?}"))?;
    let page = pdf.pages().first().ok_or("missing synthetic page")?;
    let cache = hayro::RenderCache::new();
    for round in 0..rounds {
        let start = Instant::now();
        let pixels = hayro::render(page, &cache, &InterpreterSettings::default(), &hayro::RenderSettings::default());
        let elapsed = start.elapsed();
        let hash = pixels.data_as_u8_slice().iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
        println!(
            "round={round} source={side} copies={copies} interpolate={interpolate} render_ms={:.3} hash={hash:016x}",
            elapsed.as_secs_f64() * 1000.0
        );
        if round == 0 {
            std::fs::write("target/image-cache-bench/render.rgba", pixels.data_as_u8_slice())?;
        }
    }
    Ok(())
}
