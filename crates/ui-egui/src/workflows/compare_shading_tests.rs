//! The pre-index algorithm is kept independently as an output oracle. Compare float bits so
//! NaNs and signed zero can be checked without changing their existing shading semantics.

use super::{Color32, CompareState, Comparison, DocId, Kind, VISUAL, colour, shading};
use pdfcraft_engine::compare::{Change, Side};

fn original_colour(kind: Kind) -> Color32 {
    match kind {
        Kind::Inserted => Color32::from_rgb(51, 191, 76),
        Kind::Deleted => Color32::from_rgb(229, 63, 63),
        Kind::Replaced => Color32::from_rgb(51, 114, 242),
    }
}

/// Reproduces the pre-index scan independently, including page equality and old-side deduplication.
fn original(s: &CompareState, old: bool) -> Vec<(usize, [f32; 4], Color32)> {
    let overlaps = |p: usize, r: [f32; 4], marks: &[(usize, [f32; 4], Color32)]| {
        marks.iter().any(|(mp, m, _)| *mp == p && m[0] <= r[2] && r[0] <= m[2] && m[1] <= r[3] && r[1] <= m[3])
    };
    let mut text = Vec::new();
    for ch in s.result.changes.iter().filter(|ch| !(old && ch.kind == Kind::Inserted)) {
        let side = if old { &ch.old } else { &ch.new };
        text.extend(side.rects.iter().map(|r| (side.page, r.map(|v| v as f32), original_colour(ch.kind))));
    }
    let mut visual = Vec::new();
    for (p, r) in &s.visual {
        let r = r.map(|v| v as f32);
        if !overlaps(*p, r, &text) && !(old && overlaps(*p, r, &visual)) {
            visual.push((*p, r, VISUAL));
        }
    }
    let mut out = if s.show_text { text } else { Vec::new() };
    if s.show_visual {
        out.extend(visual);
    }
    out
}

fn state() -> CompareState {
    CompareState {
        old: DocId(1),
        new: DocId(2),
        result: Comparison::default(),
        visual: Vec::new(),
        selected: None,
        side_by_side: true,
        leader: 1,
        prev_scroll: [egui::Vec2::ZERO; 2],
        show_text: true,
        show_visual: true,
    }
}

fn bits(marks: Vec<(usize, [f32; 4], Color32)>) -> Vec<(usize, [u32; 4], Color32)> {
    marks.into_iter().map(|(p, r, c)| (p, r.map(f32::to_bits), c)).collect()
}

/// Compare ordered output bits for both panes under every visibility-filter combination.
fn check(s: &mut CompareState) {
    for text in [false, true] {
        for visual in [false, true] {
            s.show_text = text;
            s.show_visual = visual;
            for old in [false, true] {
                assert_eq!(bits(shading(s, old)), bits(original(s, old)), "old={old}, text={text}, visual={visual}");
            }
        }
    }
}

#[test]
/// Preserve inclusive edges, unnormalized rectangles, non-finite casts and sparse page keys.
fn shading_preserves_edges_nonfinite_rectangles_and_sparse_pages() {
    let mut s = state();
    check(&mut s); // No pages, changes or regions.
    let rects = [
        [0.0, 0.0, 10.0, 10.0],
        [10.0, 10.0, 20.0, 20.0], // Inclusive corner contact.
        [10.0, 2.0, 10.0, 8.0],   // Degenerate line.
        [2.0, 2.0, 2.0, 2.0],
        [10.0, 10.0, -10.0, -10.0], // Reversed, never normalized.
        [-0.0, 0.0, 0.0, -0.0],
        [f64::NEG_INFINITY, -1.0, f64::INFINITY, 1.0],
        [f64::MAX, f64::MIN, f64::MAX, f64::MIN], // Overflow to f32 infinity.
        [f64::NAN, 0.0, 1.0, 1.0],
        [0.0, f64::NAN, 1.0, 1.0],
        [0.0, 0.0, f64::NAN, 1.0],
        [0.0, 0.0, 1.0, f64::NAN],
    ];
    for (i, r) in rects.into_iter().enumerate() {
        let old = Side { page: usize::MAX, rects: vec![r], ..Default::default() };
        let new = Side { page: 7, rects: vec![r], ..Default::default() };
        s.result.changes.push(Change { kind: [Kind::Inserted, Kind::Deleted, Kind::Replaced][i % 3], old, new });
        for page in [7, 0, usize::MAX, 12345] {
            s.visual.push((page, r));
        }
    }
    // Empty sides use neither the neighbouring word nor its page as a text shade.
    s.result.changes.push(Change {
        kind: Kind::Deleted,
        old: Side { page: 9, near: Some(rects[0]), ..Default::default() },
        new: Side { page: 7, near: Some(rects[0]), ..Default::default() },
    });
    check(&mut s);
}

#[test]
/// Rejected old-side regions must not enter the dedup index or reorder accepted output.
fn shading_deduplicates_only_accepted_old_regions_in_input_order() {
    let mut s = state();
    // A overlaps B, B overlaps C, A does not overlap C. Rejected B must not suppress C.
    s.visual = vec![(3, [0.0, 0.0, 2.0, 2.0]), (3, [2.0, 0.0, 4.0, 2.0]), (3, [4.0, 0.0, 6.0, 2.0]), (1, [0.0, 0.0, 2.0, 2.0])];
    assert_eq!(shading(&s, true).iter().map(|m| m.0).collect::<Vec<_>>(), [3, 3, 1]);
    assert_eq!(shading(&s, false).len(), 4);
    check(&mut s);
}

#[test]
/// Compare the oracle and indexed path on a many-page fixture and a dense single page.
fn shading_matches_original_on_many_pages_and_dense_pages() {
    for (pages, count) in [(500, 24), (1, 2400)] {
        let mut s = state();
        let mut seed = 3920_u64;
        for i in 0..pages * count {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let p = (seed as usize) % pages;
            let x = ((seed >> 32) % 600) as f64;
            let y = ((seed >> 48) % 800) as f64;
            let r = [x, y, x + 5.0, y + 9.0];
            s.result.changes.push(Change {
                kind: [Kind::Inserted, Kind::Deleted, Kind::Replaced][i % 3],
                old: Side { page: (p + 1) % pages, rects: if i % 3 == 0 { vec![] } else { vec![r] }, ..Default::default() },
                new: Side { page: p, rects: if i % 3 == 1 { vec![] } else { vec![r, r] }, ..Default::default() },
            });
            s.visual.extend([(p, r), (p, [x + 8.0, y, x + 13.0, y + 9.0])]);
        }
        check(&mut s);
    }
}

#[test]
/// Per-call indexes must reflect current documents, changes and visual regions only.
fn shading_has_no_index_left_over_after_state_or_document_changes() {
    let mut s = state();
    s.result.changes.push(Change {
        kind: Kind::Inserted,
        old: Side { page: 0, rects: vec![[0.0, 0.0, 1.0, 1.0]], ..Default::default() },
        new: Side { page: 0, rects: vec![[0.0, 0.0, 1.0, 1.0]], ..Default::default() },
    });
    s.visual.push((0, [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(shading(&s, true).first().unwrap().2, VISUAL, "old side excludes insertions");
    assert_eq!(shading(&s, false).first().unwrap().2, colour(Kind::Inserted));
    check(&mut s);
    s.old = DocId(3);
    s.new = DocId(4);
    s.result.changes.clear();
    assert_eq!(shading(&s, false).first().unwrap().2, VISUAL);
    check(&mut s);
    s.visual.clear();
    check(&mut s);
}

// Manual phase benchmark: real PDF extraction/rendering, separate from the shading-only
// release harness. No timing assertions; run explicitly with --release --ignored --nocapture.
#[test]
#[ignore = "manual Compare phase benchmark"]
/// Measure extraction, raster comparison and shading separately on synthetic documents.
fn benchmark_compare_phases() {
    use std::hint::black_box;
    use std::time::Instant;

    let lines = std::env::var("PDFCRAFT_COMPARE_LINES").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(4000).clamp(1, 20_000);
    let mut session = pdfcraft_engine::Session::new();
    let text = |word| (0..lines).map(|i| format!("L{i}a A{word} L{i}b B{word} L{i}c C{word} L{i}d D{word} L{i}e E{word}\n")).collect::<String>();
    let old = session.create_from_text("Old", &text("five")).unwrap();
    let new = session.create_from_text("New", &text("nine")).unwrap();
    let old = session.open("old.pdf", None, old, None).unwrap();
    let new = session.open("new.pdf", None, new, None).unwrap();
    // Independent artwork changes in the right margin keep the actual visual-region set
    // substantial instead of comparing text-only pages whose nearby pixels join into one box.
    let mut edits = Vec::new();
    for page in 0..session.get(new).unwrap().info.pages.len() {
        for i in 0..32 {
            let (x, y) = (480.0 + f64::from(i % 2) * 48.0, 40.0 + f64::from(i / 2) * 40.0);
            let shape = pdfcraft_engine::Shape::Rectangle { rect: [x, y, x + 8.0, y + 8.0] };
            let style = pdfcraft_engine::Style::default_for(&shape);
            edits.push(pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape,
                style,
                contents: String::new(),
                author: "Compare benchmark".into(),
            }));
        }
    }
    session.apply(new, pdfcraft_engine::Edit::Batch { label: "Benchmark artwork".into(), edits }).unwrap();
    println!("pages={}, lines={lines}; times are milliseconds; no GUI frame measurement", session.get(new).unwrap().info.pages.len());
    println!("sample,text_ms,visual_ms,original_shading_ms,indexed_shading_ms,changes,regions");
    for sample in 0..4 {
        let t = Instant::now();
        let result = session.compare(old, new).unwrap();
        let text_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let visual = session.compare_visual(old, new, 72.0).unwrap();
        let visual_ms = t.elapsed().as_secs_f64() * 1000.0;
        let mut s = state();
        s.old = old;
        s.new = new;
        s.result = result;
        s.visual = visual;
        let t = Instant::now();
        let before = (original(black_box(&s), true), original(black_box(&s), false));
        let before_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let after = (shading(black_box(&s), true), shading(black_box(&s), false));
        let after_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(bits(before.0), bits(after.0));
        assert_eq!(bits(before.1), bits(after.1));
        println!("{sample},{text_ms:.3},{visual_ms:.3},{before_ms:.3},{after_ms:.3},{},{}", s.result.changes.len(), s.visual.len());
    }
}
