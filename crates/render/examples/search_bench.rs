//! Synthetic search microbenchmark, including normalization and glyph-range mapping.
//! Run with `cargo run -p pdfcraft-render --example search_bench --locked` on both revisions
//! with the same toolchain/profile. Timings are diagnostic, never a CI deadline.
use pdfcraft_render::{PageText, TextGlyph};
use std::{hint::black_box, time::Instant};

fn main() {
    for (name, text, query) in [
        ("repeated prefix", "a".repeat(131072), format!("{}b", "a".repeat(4095))),
        ("ordinary", "The quick brown fox jumps over the lazy dog. ".repeat(3000), "brown fox".into()),
        ("unicode", "İSTANBUL κόσμος Straße العربية 日本語 ".repeat(3000), "istanbul".into()),
    ] {
        let glyphs: Vec<_> =
            text.chars().map(|c| TextGlyph { text: c.to_string(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }).collect();
        let n = glyphs.len();
        let page = PageText { glyphs, line_of: vec![0; n], space_before: vec![false; n], truncated: false };
        black_box(page.find(&query));
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            black_box(page.find(black_box(&query)));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        if let Some(median) = times.get(3) {
            println!("{name}: median {median:.3} ms (7 runs, {n} glyphs)");
        }
    }
}
