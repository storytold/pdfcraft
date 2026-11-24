//! Equality and counter tests for the label-fitting, legend-measuring and treemap rewrites.
//! The old implementations live here as references.

use std::cell::Cell;

use super::kinds::squarify;
use super::legend::swatch_w;
use super::*;
use crate::{ApproxMeasure, Measure};
use gridcraft_model::LegendPos;

/// The pre-rewrite `Ctx::fit`: one `Measure` call per candidate length, longest first.
fn fit_reference(ctx: &Ctx, s: &str, size: f32, maxw: f32) -> String {
    if ctx.tw(s, size) <= maxw {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut n = chars.len().min(400);
    while n > 0 {
        n -= 1;
        let t: String = chars.iter().take(n).collect::<String>() + "…";
        if ctx.tw(&t, size) <= maxw {
            return if n == 0 { String::new() } else { t };
        }
    }
    String::new()
}

/// Counts calls; advances differ per character (wide CJK/emoji, zero-width combining marks) and
/// "AV"-style pairs are kerned tighter, but a character never makes the text narrower.
struct Varied {
    calls: Cell<usize>,
}

impl Measure for Varied {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        self.calls.set(self.calls.get() + 1);
        let mut w = 0.0f32;
        let mut prev = ' ';
        for c in text.chars() {
            let adv = match c as u32 {
                0x300..=0x36f => 0.0,
                0x1f300..=0x1faff | 0x4e00..=0x9fff => 1.0,
                u => 0.3 + (u % 7) as f32 * 0.07,
            };
            let kern = if prev == 'A' && c == 'V' { 0.1 } else { 0.0 };
            w += (adv - kern) * size;
            prev = c;
        }
        if bold { w * 1.1 } else { w }
    }
}

struct Counting<'a>(&'a dyn Measure, Cell<usize>);

impl Measure for Counting<'_> {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        self.1.set(self.1.get() + 1);
        self.0.text_width(text, size, bold)
    }
}

/// Like a text layout that breaks at `\n`: the width is that of the widest line. Adding a
/// newline can make the text narrower, so the width of `prefix + "…"` is not monotone.
struct WidestLine;

impl Measure for WidestLine {
    fn text_width(&self, text: &str, size: f32, _bold: bool) -> f32 {
        text.split('\n')
            .map(|line| {
                line.chars()
                    .map(|c| {
                        size * if c == 'i' {
                            0.25
                        } else if c == 'M' {
                            0.9
                        } else {
                            0.5
                        }
                    })
                    .sum::<f32>()
            })
            .fold(0.0, f32::max)
    }
}

fn corpus() -> Vec<String> {
    let mut v: Vec<String> =
        ["", "a", "ab", "Hello, world", "AVAVAV tight", "日本語のラベルです", "👨‍👩‍👧 family 🎉 party", "e\u{301}e\u{301}e\u{301} combining", "…", "……"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    v.push("x".repeat(399));
    v.push("x".repeat(400));
    v.push("x".repeat(401));
    v.push("é🎉".repeat(500));
    v.push("long label ".repeat(100));
    for s in ["iiii\nMM", "iiii\n", "\niiii", "ab\ncd\nef", "iiiiiiii\niiiiMMMM\nMM", "line one\nline two is longer\nx"] {
        v.push(s.to_string());
    }
    v
}

fn run_fit(m: &dyn Measure) {
    let ctx = Ctx { out: vec![], m };
    for s in corpus() {
        for size in [9.0f32, 11.0, 40.0] {
            for maxw in [-5.0, 0.0, 0.4, 6.0, 25.0, 61.5, 120.0, 800.0, 1e6, f32::INFINITY, f32::NAN] {
                assert_eq!(ctx.fit(&s, size, maxw), fit_reference(&ctx, &s, size, maxw), "{s:?} size {size} maxw {maxw}");
            }
        }
    }
}

#[test]
fn fit_matches_linear_reference() {
    run_fit(&ApproxMeasure);
    run_fit(&Varied { calls: Cell::new(0) });
    run_fit(&WidestLine);
}

#[test]
fn a_multiline_label_keeps_its_fitting_prefix() {
    // The whole label is 18 wide and "iii…" is 12.5, both too wide for 10.5, but "iiii\n…" is
    // 10 wide and fits. A binary search that stops at the failing midpoint would return nothing.
    let ctx = Ctx { out: vec![], m: &WidestLine };
    let s = "iiii\nMM";
    let size = 10.0;
    let maxw = 10.5;
    assert!(ctx.tw(s, size) > maxw);
    assert!(ctx.tw("iii…", size) > maxw && ctx.tw("iiii\n…", size) <= maxw);
    assert_eq!(ctx.fit(s, size, maxw), "iiii\n…");
}

#[test]
fn fit_measures_logarithmically() {
    let long = "x".repeat(1000);
    let c = Counting(&ApproxMeasure, Cell::new(0));
    let ctx = Ctx { out: vec![], m: &c };
    let r = ctx.fit(&long, 11.0, 60.0);
    assert!(r.ends_with('…') && r.chars().count() > 3);
    // 1 whole-string check + ceil(log2(399)) + 1 probes; the old scan needed ~390 here.
    assert!(c.1.get() <= 12, "{} Measure calls", c.1.get());
    let c2 = Counting(&ApproxMeasure, Cell::new(0));
    let ctx2 = Ctx { out: vec![], m: &c2 };
    assert_eq!(ctx2.fit("short", 11.0, 500.0), "short");
    assert_eq!(c2.1.get(), 1);
}

fn legend_entries(n: usize, label: &dyn Fn(usize) -> String) -> Vec<(String, Rgba, Swatch)> {
    (0..n).map(|i| (label(i), [10, 20, 30, 255], if i % 3 == 0 { Swatch::Line } else { Swatch::Box })).collect()
}

/// Colour column width as the old code computed it, from the returned remaining area.
fn old_colw(m: &dyn Measure, entries: &[(String, Rgba, Swatch)], area: Bx) -> f32 {
    let ctx = Ctx { out: vec![], m };
    let size = LABEL_SIZE;
    let s = size * 0.8;
    entries.iter().map(|e| swatch_w(e.2, s) + 4.0 + ctx.tw(&e.0, size)).fold(0.0f32, f32::max).min(area.w * 0.35)
}

#[test]
fn side_legend_matches_old_column_width_and_stops_measuring_at_the_cap() {
    let area = Bx { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
    let labels: [&dyn Fn(usize) -> String; 3] =
        [&|i| format!("entry {i}"), &|i| if i == 7 { "w".repeat(80) } else { format!("e{i}") }, &|i| "very long legend label ".repeat(1 + i % 3)];
    for pos in [LegendPos::Left, LegendPos::Right] {
        for (k, label) in labels.iter().enumerate() {
            for n in [0usize, 1, 5, 40] {
                let entries = legend_entries(n, label);
                let mut ctx = Ctx { out: vec![], m: &ApproxMeasure };
                let rest = draw_legend(&mut ctx, &entries, pos, area);
                let colw = old_colw(&ApproxMeasure, &entries, area);
                assert!((rest.w - (area.w - (colw + 8.0))).abs() < 1e-4, "{pos:?} labels {k} n {n}");
            }
        }
    }
    // 20,000 entries whose first one already fills the 35% cap: the old code measured all of them
    // plus the drawn rows, now only the first entry and the drawn rows are measured.
    let entries = legend_entries(20_000, &|i| if i == 0 { "w".repeat(200) } else { format!("e{i}") });
    let c = Counting(&ApproxMeasure, Cell::new(0));
    let mut ctx = Ctx { out: vec![], m: &c };
    let _ = draw_legend(&mut ctx, &entries, LegendPos::Right, area);
    let drawn = ctx.out.iter().filter(|p| matches!(p, Prim::Text { .. })).count();
    assert!(c.1.get() <= drawn * 10 + 5, "{} Measure calls for {drawn} drawn entries", c.1.get());
    assert!(c.1.get() < 500);
}

/// The pre-rewrite `worst`/`squarify` (a `Vec` per item, row recomputed from scratch).
fn worst_reference(row: &[f64], side: f64) -> f64 {
    let s: f64 = row.iter().sum();
    if s <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let (mn, mx) = row.iter().fold((f64::INFINITY, 0.0f64), |(a, b), v| (a.min(*v), b.max(*v)));
    let s2 = s * s;
    let w2 = side * side;
    (w2 * mx / s2).max(s2 / (w2 * mn))
}

fn squarify_reference(items: &[(usize, f64)], r: Bx) -> Vec<(usize, Bx)> {
    let total: f64 = items.iter().map(|i| i.1).sum();
    let area = (r.w as f64) * (r.h as f64);
    if !(total > 0.0 && area > 0.0) {
        return vec![];
    }
    let scale = area / total;
    let mut out = Vec::with_capacity(items.len());
    let mut rem = r;
    let mut row: Vec<(usize, f64)> = Vec::new();
    let mut i = 0;
    let layout = |row: &[(usize, f64)], rem: &mut Bx, out: &mut Vec<(usize, Bx)>| {
        let s: f64 = row.iter().map(|x| x.1).sum();
        if s <= 0.0 {
            return;
        }
        if rem.w >= rem.h {
            let cw = (s / rem.h.max(1e-6) as f64) as f32;
            let mut y = rem.y;
            for (j, a) in row {
                let h = (*a / cw.max(1e-6) as f64) as f32;
                out.push((*j, Bx { x: rem.x, y, w: cw, h }));
                y += h;
            }
            rem.x += cw;
            rem.w = (rem.w - cw).max(0.0);
        } else {
            let rh = (s / rem.w.max(1e-6) as f64) as f32;
            let mut x = rem.x;
            for (j, a) in row {
                let w = (*a / rh.max(1e-6) as f64) as f32;
                out.push((*j, Bx { x, y: rem.y, w, h: rh }));
                x += w;
            }
            rem.y += rh;
            rem.h = (rem.h - rh).max(0.0);
        }
    };
    while let Some(&(j, v)) = items.get(i) {
        let a = v * scale;
        let side = rem.w.min(rem.h) as f64;
        let cur: Vec<f64> = row.iter().map(|x| x.1).collect();
        let mut with = cur.clone();
        with.push(a);
        if row.is_empty() || worst_reference(&with, side) <= worst_reference(&cur, side) {
            row.push((j, a));
            i += 1;
        } else {
            layout(&row, &mut rem, &mut out);
            row.clear();
        }
    }
    if !row.is_empty() {
        layout(&row, &mut rem, &mut out);
    }
    out
}

#[test]
fn squarify_matches_reference() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut cases: Vec<Vec<f64>> = vec![
        vec![],
        vec![5.0],
        vec![6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0],
        vec![3.0, 0.0, 0.0, 1.0],
        vec![0.0, 0.0],
        vec![4.0, -1.0, 2.0],
        vec![4.0, f64::NAN, 2.0, 1.0],
        vec![f64::INFINITY, 1.0],
        vec![1e-300, 1e-310, 0.0],
    ];
    for n in [2usize, 9, 40, 300] {
        let mut v: Vec<f64> = (0..n).map(|_| rnd() * 100.0).collect();
        v.sort_by(|a, b| b.total_cmp(a));
        cases.push(v);
    }
    for r in [Bx { x: 0.0, y: 0.0, w: 300.0, h: 200.0 }, Bx { x: 5.0, y: 7.0, w: 90.0, h: 400.0 }, Bx { x: 0.0, y: 0.0, w: 0.0, h: 50.0 }] {
        for vals in &cases {
            let items: Vec<(usize, f64)> = vals.iter().copied().enumerate().collect();
            let new: Vec<(usize, Bx)> = squarify(&items, r)
                .into_iter()
                .map(|(j, v, b)| {
                    assert!(vals.get(j).is_some_and(|x| x.to_bits() == v.to_bits()), "value carried through");
                    (j, b)
                })
                .collect();
            let old = squarify_reference(&items, r);
            assert_eq!(format!("{new:?}"), format!("{old:?}"), "{vals:?} in {r:?}");
        }
    }
}
