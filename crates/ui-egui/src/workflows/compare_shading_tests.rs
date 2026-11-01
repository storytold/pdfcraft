//! The pre-index algorithm is kept independently as an output oracle. Compare float bits so
//! NaNs and signed zero can be checked without changing their existing shading semantics.

use super::{Color32, CompareState, Comparison, DocId, Kind, VISUAL, colour, shading};
use pdfcraft_engine::compare::{Change, Side};
use std::collections::HashMap;

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

#[derive(Default, Debug)]
struct WorkCounts {
    same_page_rect_tests: usize,
    page_key_checks: usize,
    page_lookups: usize,
    indexed_entries: usize,
    nonempty_buckets: usize,
}

/// Count source-matched overlap work and index overhead; this model does not instrument production.
fn count_work(s: &CompareState, old: bool) -> (WorkCounts, WorkCounts) {
    let mut text = Vec::new();
    for ch in s.result.changes.iter().filter(|ch| !(old && ch.kind == Kind::Inserted)) {
        let side = if old { &ch.old } else { &ch.new };
        text.extend(side.rects.iter().map(|r| (side.page, r.map(|v| v as f32))));
    }
    let visual: Vec<_> = s.visual.iter().map(|(p, r)| (*p, r.map(|v| v as f32))).collect();
    let overlaps = |a: &[f32; 4], b: &[f32; 4]| a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
    let mut scan = WorkCounts::default();
    let mut accepted = Vec::new();
    for (page, rect) in &visual {
        let mut hit = false;
        for (mark_page, mark) in &text {
            scan.page_key_checks += 1;
            if page == mark_page {
                scan.same_page_rect_tests += 1;
                if overlaps(mark, rect) {
                    hit = true;
                    break;
                }
            }
        }
        if !hit && old {
            for (mark_page, mark) in &accepted {
                scan.page_key_checks += 1;
                if page == mark_page {
                    scan.same_page_rect_tests += 1;
                    if overlaps(mark, rect) {
                        hit = true;
                        break;
                    }
                }
            }
        }
        if !hit {
            accepted.push((*page, *rect));
        }
    }

    let mut indexed = WorkCounts::default();
    let mut text_pages: HashMap<usize, Vec<[f32; 4]>> = HashMap::new();
    for (page, rect) in &text {
        text_pages.entry(*page).or_default().push(*rect);
    }
    indexed.indexed_entries += text.len();
    indexed.nonempty_buckets += text_pages.len();
    let mut accepted_pages: HashMap<usize, Vec<[f32; 4]>> = HashMap::new();
    for (page, rect) in &visual {
        indexed.page_lookups += 1;
        let text_hit = text_pages.get(page).is_some_and(|marks| {
            marks.iter().any(|mark| {
                indexed.same_page_rect_tests += 1;
                overlaps(mark, rect)
            })
        });
        if !text_hit {
            let visual_hit = if old {
                indexed.page_lookups += 1;
                accepted_pages.get(page).is_some_and(|marks| {
                    marks.iter().any(|mark| {
                        indexed.same_page_rect_tests += 1;
                        overlaps(mark, rect)
                    })
                })
            } else {
                false
            };
            if visual_hit {
                continue;
            }
            if old {
                accepted_pages.entry(*page).or_default().push(*rect);
                indexed.indexed_entries += 1;
            }
        }
    }
    indexed.nonempty_buckets += accepted_pages.len();
    // Compare actual output before treating the modeled work counts as relevant evidence.
    let actual = bits(shading(s, old));
    let oracle = bits(original(s, old));
    assert_eq!(actual, oracle, "counter model input must satisfy shading oracle");
    (scan, indexed)
}

#[test]
/// Compare bit-identical shading and deterministic work counts on bounded page distributions.
fn shading_operation_counts_are_page_local() {
    let mut exact = state();
    let r = [0.0, 0.0, 2.0, 2.0];
    exact.result.changes.push(Change {
        kind: Kind::Replaced,
        old: Side { page: 7, rects: vec![r], ..Default::default() },
        new: Side { page: 7, rects: vec![r], ..Default::default() },
    });
    exact.visual.push((7, r));
    let (scan, indexed) = count_work(&exact, false);
    assert_eq!((scan.same_page_rect_tests, scan.page_key_checks), (1, 1));
    assert_eq!((indexed.same_page_rect_tests, indexed.page_lookups, indexed.indexed_entries, indexed.nonempty_buckets), (1, 1, 1, 1));

    let mut rows = Vec::new();
    for (name, pages, marks_per_page, overlap) in [
        ("sparse_200_pages", 200, 1, false),
        ("dense_one_page", 1, 80, false),
        ("repeated_overlapping_pages", 25, 8, true),
        ("empty", 5000, 0, false),
    ] {
        let mut s = state();
        for p in 0..pages {
            for i in 0..marks_per_page {
                let x = if overlap { 0.0 } else { (i * 4) as f64 };
                let r = [x, 0.0, x + 2.0, 2.0];
                s.result.changes.push(Change {
                    kind: Kind::Replaced,
                    old: Side { page: p, rects: vec![r], ..Default::default() },
                    new: Side { page: p, rects: vec![r], ..Default::default() },
                });
                s.visual.push((p, r));
                s.visual.push((p, r));
            }
        }
        check(&mut s);
        for text in [true] {
            for visual in [false, true] {
                s.show_text = text;
                s.show_visual = visual;
                for old in [false, true] {
                    let (scan, indexed) = count_work(&s, old);
                    rows.push(format!(
                        "{name},{pages},{marks_per_page},{overlap},{text},{visual},{old},{},{},{},{},{},{}",
                        scan.same_page_rect_tests,
                        scan.page_key_checks,
                        indexed.same_page_rect_tests,
                        indexed.page_lookups,
                        indexed.indexed_entries,
                        indexed.nonempty_buckets
                    ));
                }
            }
        }
    }
    println!(
        "workload,pages,marks_per_page,overlap,show_text,show_visual,old,scan_rect_tests,scan_page_checks,indexed_rect_tests,page_lookups,index_entries,buckets"
    );
    for row in rows {
        println!("{row}");
    }
}

#[test]
#[ignore = "manual 5,000-page-identifier counter model; this is not a PDF test"]
/// Measure bounded in-memory candidate counts across 5,000 sparse page identifiers.
fn shading_operation_counts_for_5000_page_identifiers() {
    let mut s = state();
    for page in 0..5000 {
        let x = page as f64 * 4.0;
        let rect = [x, 0.0, x + 2.0, 2.0];
        s.result.changes.push(Change {
            kind: Kind::Replaced,
            old: Side { page, rects: vec![rect], ..Default::default() },
            new: Side { page, rects: vec![rect], ..Default::default() },
        });
        s.visual.push((page, rect));
    }
    let (scan, indexed) = count_work(&s, false);
    assert_eq!(bits(shading(&s, false)), bits(original(&s, false)));
    println!(
        "pages=5000,scan_same_page={},scan_page_checks={},indexed_same_page={},lookups={},entries={},buckets={}",
        scan.same_page_rect_tests,
        scan.page_key_checks,
        indexed.same_page_rect_tests,
        indexed.page_lookups,
        indexed.indexed_entries,
        indexed.nonempty_buckets
    );
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
/// Exercise bounded, reproducible mixtures of sparse pages, repeated marks and change kinds.
fn shading_matches_original_across_fixed_seeds() {
    for initial in [0_u64, 1, 17, 3920, u64::MAX] {
        let mut seed = initial;
        let mut s = state();
        for i in 0..120 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let page = [0, 3, 97, 4999, usize::MAX][(seed as usize) % 5];
            let x = ((seed >> 32) % 20) as f64 * 4.0;
            let y = ((seed >> 48) % 20) as f64 * 4.0;
            let r = [x, y, x + 6.0, y + 6.0];
            s.result.changes.push(Change {
                kind: [Kind::Inserted, Kind::Deleted, Kind::Replaced][i % 3],
                old: Side { page, rects: if i % 4 == 0 { vec![] } else { vec![r] }, ..Default::default() },
                new: Side { page: if i % 2 == 0 { page } else { 42 }, rects: if i % 5 == 0 { vec![] } else { vec![r, r] }, ..Default::default() },
            });
            s.visual.extend([(page, r), (page, r), (page, [x + 20.0, y, x + 22.0, y + 2.0])]);
        }
        for text in [false, true] {
            for visual in [false, true] {
                s.show_text = text;
                s.show_visual = visual;
                for old in [false, true] {
                    assert_eq!(bits(shading(&s, old)), bits(original(&s, old)), "seed={initial}, old={old}, text={text}, visual={visual}");
                }
            }
        }
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
