use std::collections::BTreeSet;

use super::*;
use crate::combine_ui::{moved_order, step_to};

#[test]
fn long_names_keep_their_extension_and_whole_characters() {
    let fits = |n: usize| move |s: &str| s.chars().count() <= n;
    assert_eq!(fit_name("short.pdf", fits(20)), "short.pdf");
    assert_eq!(fit_name("# 227 Pilot Boat Advance.pdf", fits(18)), "# 227 Pilot B….pdf");
    // Multi-byte characters are never cut in half.
    assert_eq!(fit_name("Überweisungsbestätigung.pdf", fits(12)), "Überwei….pdf");
    assert_eq!(fit_name("日本語のファイル名です.pdf", fits(8)), "日本語….pdf");
    // No extension, or a dot that isn't one.
    assert_eq!(fit_name("README-without-extension", fits(10)), "README-wi…");
    assert_eq!(fit_name(".hidden-file-name", fits(8)), ".hidden…");
    // Nothing fits: the ellipsis and the extension.
    assert_eq!(fit_name("abcdef.pdf", fits(0)), "….pdf");
}

#[test]
fn paper_sizes_fit_the_box_and_bad_sizes_are_refused() {
    // The box follows the zoom, within its range; nonsense is 100%.
    assert_eq!(paper_box(1.0), PAPER);
    assert_eq!(paper_box(2.0), PAPER * 2.0);
    assert_eq!(paper_box(99.0), PAPER * 5.0);
    assert_eq!(paper_box(-1.0), PAPER * 0.6);
    assert_eq!(paper_box(f32::NAN), PAPER);
    for zoom in [0.6, 1.0, 2.0, 5.0] {
        let fit = paper_box(zoom);
        // Letter is a little wider than the box: it fills the width. A tall page fills the height.
        let letter = paper_size(612.0, 792.0, fit).unwrap();
        assert!((letter.x - fit.x).abs() < 0.01 && letter.y <= fit.y + 0.01, "{letter:?}");
        let tall = paper_size(612.0, 1008.0, fit).unwrap();
        assert!((tall.y - fit.y).abs() < 0.01 && tall.x <= fit.x + 0.01, "{tall:?}");
        let landscape = paper_size(792.0, 612.0, fit).unwrap();
        assert!((landscape.x - fit.x).abs() < 0.01 && landscape.y < fit.y, "{landscape:?}");
        for (w, h) in [(0.0, 792.0), (612.0, -1.0), (f32::NAN, 1.0), (f32::INFINITY, 1.0), (1e-30, 1e30)] {
            let s = paper_size(w, h, fit);
            assert!(s.is_none_or(|s| s.x.is_finite() && s.y.is_finite() && s.x <= fit.x + 0.01 && s.y <= fit.y + 0.01), "{w}×{h}: {s:?}");
        }
    }
}

#[test]
fn zoom_buttons_step_through_round_sizes() {
    let mut z = 0.6;
    let mut up = vec![z];
    while z < 5.0 {
        z = zoom_step(z, true);
        up.push(z);
    }
    assert_eq!(up, ZOOM_STEPS);
    assert_eq!(zoom_step(5.0, true), 5.0, "the largest stays put");
    assert_eq!(zoom_step(0.6, false), 0.6, "the smallest stays put");
    // From anywhere the slider left it, to the next round size.
    assert_eq!(zoom_step(1.1, true), 1.25);
    assert_eq!(zoom_step(1.1, false), 1.0);
    assert_eq!(zoom_step(1.953, true), 2.0);
    assert_eq!(zoom_step(1.0000001, true), 1.25, "a hair over a size counts as that size");
}

#[test]
fn the_cards_zoom_far_enough_to_read_a_value() {
    // #2531: the grid stopped at 200%, which was not enough to read invoice values while
    // reordering the pages of a statement. It has to reach 500%.
    assert_eq!(*ZOOM_RANGE.end(), 5.0, "the grid stops at {}%, not 500%", ZOOM_RANGE.end() * 100.0);
    assert_eq!(paper_box(5.0), PAPER * 5.0, "500% is kept, not clamped to the old ceiling");
    // Every round size above the old 200% ceiling is a step the toolbar can reach.
    let mut z = *ZOOM_RANGE.start();
    let mut up = vec![z];
    while z < *ZOOM_RANGE.end() {
        z = zoom_step(z, true);
        up.push(z);
    }
    for want in [2.5, 3.0, 4.0, 5.0] {
        assert!(up.contains(&want), "no step reaches {want}%: {up:?}");
    }
    // A card stays sharp at the ceiling instead of upscaled from the old 768px cap. A Letter
    // page fills the height of the box, so at 500% it needs PAPER.y * 5.0 points of it, which
    // on a 1.5 pixels-per-point screen is this many pixels.
    let wanted = PAPER.y * 5.0 * 1.5;
    assert!(MAX_SIDE >= wanted, "MAX_SIDE {MAX_SIDE} is under the {wanted}px a card needs at 500%");
}

#[test]
fn arrow_keys_step_by_file_and_by_row() {
    // The list: one at a time, staying put at both ends.
    assert_eq!(step_to(0, 5, 1, false), 0);
    assert_eq!(step_to(4, 5, 1, true), 4);
    assert_eq!(step_to(2, 5, 1, true), 3);
    // A grid of 3 per row and 8 files: rows [0 1 2] [3 4 5] [6 7].
    assert_eq!(step_to(1, 8, 3, true), 4, "down one row, same column");
    assert_eq!(step_to(5, 8, 3, true), 7, "a short last row is reached at its last file");
    assert_eq!(step_to(6, 8, 3, true), 6, "the last row goes no further down");
    assert_eq!(step_to(2, 8, 3, false), 2, "the first row goes no further up");
    assert_eq!(step_to(7, 8, 3, false), 4);
    // Out-of-range starts and empty lists don't panic.
    assert_eq!(step_to(99, 8, 3, false), 4);
    assert_eq!(step_to(0, 0, 3, true), 0);
    assert_eq!(step_to(0, 1, 0, true), 0);
}

#[test]
fn moves_to_a_gap_keep_order_and_report_no_ops() {
    let ids = [10, 11, 12, 13, 14];
    let set = |v: &[u64]| v.iter().copied().collect::<BTreeSet<u64>>();
    // One file to the start, to the end, and between others.
    assert_eq!(moved_order(&ids, &set(&[13]), 0), Some(vec![13, 10, 11, 12, 14]));
    assert_eq!(moved_order(&ids, &set(&[10]), 5), Some(vec![11, 12, 13, 14, 10]));
    assert_eq!(moved_order(&ids, &set(&[10]), 3), Some(vec![11, 12, 10, 13, 14]));
    // A discontiguous selection gathers at the gap, in list order.
    assert_eq!(moved_order(&ids, &set(&[10, 12, 14]), 2), Some(vec![11, 10, 12, 14, 13]));
    assert_eq!(moved_order(&ids, &set(&[11, 13]), 5), Some(vec![10, 12, 14, 11, 13]));
    // Dropping a file next to itself, or a block inside itself, changes nothing.
    assert_eq!(moved_order(&ids, &set(&[12]), 2), None);
    assert_eq!(moved_order(&ids, &set(&[12]), 3), None);
    assert_eq!(moved_order(&ids, &set(&[11, 12, 13]), 2), None);
    // Nothing to move, unknown files and gaps past the end are harmless.
    assert_eq!(moved_order(&ids, &set(&[]), 1), None);
    assert_eq!(moved_order(&ids, &set(&[99]), 1), None);
    assert_eq!(moved_order(&ids, &set(&[10]), 99), Some(vec![11, 12, 13, 14, 10]));
}

/// A PDF of `pages` pages of `w` × `h` points.
fn pdf(pages: usize, w: u32, h: u32) -> Arc<Vec<u8>> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 {w} {h}] >>", kids.join(" ")));
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 4 + 2 * i));
        let body = "0.2 0.4 0.8 rg 0 0 10 10 re f";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Arc::new(out)
}

fn key_of(bytes: &Arc<Vec<u8>>) -> Key {
    Key { source: Arc::as_ptr(bytes) as usize, auth: 1, page: 0, px: [32, 48] }
}

fn want(file: u64, bytes: &Arc<Vec<u8>>) -> Want {
    Want { slot: (file, 0), key: key_of(bytes), bytes: bytes.clone(), password: None, scale: 0.15, visible: true }
}

fn listing(files: &[(u64, &Arc<Vec<u8>>)]) -> HashMap<Slot, Key> {
    files.iter().map(|(f, b)| ((*f, 0), key_of(b))).collect()
}

/// One frame of `finish`, as the app runs it; how soon it asks to be drawn again.
fn frame(ctx: &egui::Context, thumbs: &mut Thumbs, current: &HashMap<Slot, Key>) -> std::time::Duration {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| thumbs.finish(ui.ctx(), &|slot| current.get(&slot).copied()));
    // No GPU here: the texture uploads are taken as done.
    out.textures_delta.clear();
    out.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay)
}

#[test]
fn a_thumbnail_that_arrives_is_drawn_without_waiting_for_input() {
    // Regression: the cards are drawn before results are taken; the last result used to
    // leave its card blank until the next mouse move.
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    let bytes = pdf(1, 200, 300);
    let current = listing(&[(1, &bytes)]);
    thumbs.wants.push(want(1, &bytes));
    frame(&ctx, &mut thumbs, &current);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let delay = frame(&ctx, &mut thumbs, &current);
        if thumbs.entries.get(&(1, 0)).is_some_and(|e| e.tex.is_some()) {
            assert_eq!(delay, std::time::Duration::ZERO, "the frame that took the result asks for another");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the thumbnail arrives");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_result_for_a_file_that_changed_is_never_shown() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    let bytes = pdf(1, 200, 300);
    thumbs.wants.push(want(1, &bytes));
    frame(&ctx, &mut thumbs, &listing(&[(1, &bytes)]));
    assert_eq!(thumbs.jobs.len(), 1, "the render started");
    // The file leaves the list (or is unlocked differently) while it renders.
    let other = pdf(1, 200, 300);
    let changed = listing(&[(1, &other)]);
    frame(&ctx, &mut thumbs, &changed);
    assert!(thumbs.jobs.is_empty(), "let go of at once");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while thumbs.threads() > 0 {
        assert!(std::time::Instant::now() < deadline, "its thread exits");
        frame(&ctx, &mut thumbs, &changed);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(thumbs.entries.is_empty(), "its result is never kept");
}

#[test]
fn textures_over_budget_go_longest_unseen_first_and_those_in_view_stay() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    let tex = |n: &str| ctx.load_texture(n, egui::ColorImage::new([1, 1], vec![Color32::WHITE]), egui::TextureOptions::LINEAR);
    let files: Vec<Arc<Vec<u8>>> = (0..4).map(|_| pdf(1, 200, 300)).collect();
    thumbs.frame = 10;
    // Seen at frames 1, 5 and 9 (out of view now), and 10 (in view), 20 MB each as counted.
    for (i, seen) in [1u64, 5, 9, 10].into_iter().enumerate() {
        let key = key_of(&files[i]);
        thumbs.entries.insert((i as u64, 0), Entry { key, tex: Some(tex(&format!("t{i}"))), error: None, seen, bytes: 20 << 20 });
    }
    let current: HashMap<Slot, Key> = files.iter().enumerate().map(|(i, b)| ((i as u64, 0), key_of(b))).collect();
    frame(&ctx, &mut thumbs, &current);
    let mut kept: Vec<u64> = thumbs.entries.keys().map(|k| k.0).collect();
    kept.sort_unstable();
    assert_eq!(kept, [3], "80 MB: the three out of view go, oldest first; the one in view stays even over budget");
}

#[test]
fn render_threads_never_exceed_three_while_files_come_and_go() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    let files: Vec<Arc<Vec<u8>>> = (0..12).map(|i| pdf(1 + i % 3, 200, 300)).collect();
    let mut most = 0;
    for round in 0..120u64 {
        // A different handful of files every few frames: renders are abandoned all the time.
        let start = (round / 3 % 4 * 3) as usize;
        let shown: Vec<(u64, &Arc<Vec<u8>>)> = (start..start + 4).map(|i| (i as u64, &files[i % files.len()])).collect();
        for (file, bytes) in &shown {
            thumbs.wants.push(want(*file, bytes));
        }
        frame(&ctx, &mut thumbs, &listing(&shown));
        let threads = thumbs.threads();
        most = most.max(threads);
        assert!(threads <= MAX_THREADS, "round {round}: {threads} render threads");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(most > 0, "renders did run");
}

#[test]
fn on_the_web_one_thumbnail_waits_at_a_time() {
    // Regression: pools that render on the UI thread have no threads to count, so three
    // started every frame while one finished, each holding its parsed document.
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs { inline: true, ..Default::default() };
    let files: Vec<Arc<Vec<u8>>> = (0..6).map(|_| pdf(1, 200, 300)).collect();
    let shown: Vec<(u64, &Arc<Vec<u8>>)> = files.iter().enumerate().map(|(i, b)| (i as u64, b)).collect();
    // One heavy step a frame on the UI thread: a parse, then the next frame its render.
    for round in 0..14 {
        for (file, bytes) in &shown {
            thumbs.wants.push(want(*file, bytes));
        }
        frame(&ctx, &mut thumbs, &listing(&shown));
        assert!(thumbs.jobs.len() <= 1, "round {round}: {} renders waiting", thumbs.jobs.len());
        assert!(thumbs.jobs.iter().all(|j| j.pool.is_inline()));
    }
    assert!(thumbs.entries.values().filter(|e| e.tex.is_some()).count() == 6, "all six drawn, one heavy step per frame");
}

#[test]
fn the_next_page_of_a_file_reuses_its_open_document_until_it_closes() {
    // Regression: each page of an expanded file read (parsed) the whole file again.
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    let bytes = pdf(3, 200, 300);
    let first = key_of(&bytes);
    let second = Key { page: 1, ..first };
    let current: HashMap<Slot, Key> = [((1, 0), first), ((1, 1), second)].into_iter().collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let drawn = |thumbs: &Thumbs, slot: Slot| thumbs.entries.get(&slot).is_some_and(|e| e.tex.is_some());
    while !drawn(&thumbs, (1, 0)) {
        assert!(std::time::Instant::now() < deadline, "page 1 is drawn");
        thumbs.wants.push(want(1, &bytes));
        frame(&ctx, &mut thumbs, &current);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(thumbs.open.len(), 1, "its document stays open a moment");
    // Page 2 of the same file, read the same way: that open document renders it.
    thumbs.wants.push(Want { slot: (1, 1), key: second, ..want(1, &bytes) });
    frame(&ctx, &mut thumbs, &current);
    assert!(thumbs.open.is_empty() && thumbs.jobs.len() == 1, "reused, not a new pool: {} open, {} jobs", thumbs.open.len(), thumbs.jobs.len());
    while !drawn(&thumbs, (1, 1)) {
        assert!(std::time::Instant::now() < deadline, "page 2 is drawn");
        frame(&ctx, &mut thumbs, &current);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(thumbs.threads() <= MAX_THREADS);
    // Unused, it closes after a while, and its thread goes.
    while thumbs.threads() > 0 || !thumbs.open.is_empty() {
        assert!(std::time::Instant::now() < deadline, "the open document closes and its thread exits");
        frame(&ctx, &mut thumbs, &current);
    }
}

#[test]
fn a_page_that_failed_is_not_tried_again_at_another_size() {
    // Regression: each zoom step asked for a new render of a page that had failed (or hung
    // for the watchdog's 20 s), filling the render threads.
    // (A card asks for a render only when it looks Pending.)
    let mut thumbs = Thumbs::default();
    let bytes = pdf(1, 200, 300);
    let key = key_of(&bytes);
    thumbs.entries.insert((1, 0), Entry { key, tex: None, error: Some("broken".into()), seen: 0, bytes: 0 });
    let larger = Key { px: [key.px[0] + 64, key.px[1] + 64], ..key };
    assert!(matches!(thumbs.look((1, 0), larger), Look::Failed(e) if e == "broken"), "the failure stands at another size");
    // Another read of the file (a new password) is a new page: tried again.
    let reread = Key { auth: 2, ..key };
    assert!(matches!(thumbs.look((1, 0), reread), Look::Pending(None)));
}

#[test]
fn on_the_web_a_render_that_never_answers_fails_and_lets_the_rest_through() {
    // A pool without threads answers on the call that renders: no answer then never comes.
    // It used to wait for good, so nothing else started and every frame asked for another.
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs { inline: true, ..Default::default() };
    let (silent, next) = (pdf(1, 200, 300), pdf(1, 200, 300));
    let pool = RenderPool::new_inline(silent.clone(), RenderConfig::default());
    // Nothing queued: this pool has nothing to answer with.
    thumbs.jobs.push(Job { slot: (1, 0), key: key_of(&silent), pool, tag: 7 });
    let current = listing(&[(1, &silent), (2, &next)]);
    for _ in 0..4 {
        thumbs.wants.push(want(2, &next));
        frame(&ctx, &mut thumbs, &current);
    }
    assert!(thumbs.entries.get(&(1, 0)).is_some_and(|e| e.tex.is_none() && e.error.is_some()), "the silent one failed");
    assert!(thumbs.entries.get(&(2, 0)).is_some_and(|e| e.tex.is_some()), "the next one was drawn");
    assert!(thumbs.jobs.is_empty());
    assert!(frame(&ctx, &mut thumbs, &current) > std::time::Duration::ZERO, "no more frames asked for at once");
}

/// Combine thumbnails are on-screen rasters: like the viewer, they may draw a non-embedded
/// CJK font with an installed face, and they keep the file's password.
#[test]
fn thumbnails_use_the_on_screen_configuration() {
    let config = thumbnail_config(Some(Arc::from("secret")));
    assert!(config.system_fonts, "Combine thumbnails draw non-embedded CJK text like the viewer");
    assert_eq!(config.password.as_deref(), Some("secret"));
    assert!(!RenderConfig::default().system_fonts, "anything else stays off by default");
}
