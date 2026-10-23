//! #3634: export, OCR and print — and every other path that reads pages without the viewer's pool,
//! like compare, word count and the previews — go through [`PageRenderer`], which had no per-page time
//! limit: a page the viewer's pool gives up on after twenty seconds could keep those callers busy for
//! as long as it took. `RenderConfig` now carries a budget — [`PAGE_TIME_LIMIT`], sixty
//! seconds, since these callers wait rather than paint a window — so every path that draws pages
//! inherits it, and `PageRenderer` arms a timer that raises the
//! interpreter's existing `cancelled` flag — the same one [`RenderPool`]'s watchdog uses — and
//! reports the page with the pool's own wording.
//!
//! Without that enforcement every "skipped" expectation below fails with "the page was drawn", and
//! the one test that measures work (`…stops_at_its_next_operator…`) fails because a bounded render
//! takes as long as an unbounded one. Zero and five-millisecond budgets keep the others
//! deterministic: no test waits on a pathological document, none needs craft-fonts.

#![cfg(not(target_arch = "wasm32"))]

use crate::raster::PageTimeLimit;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{ONE_PAGE, ONE_PAGE_TWICE};
use crate::ReadingColors;
use crate::raster::{NO_PAGE_TIME_LIMIT, PAGE_TIME_LIMIT, PageRenderer, RenderConfig, RenderPool, RenderRequest, RequestKind, STUCK_AFTER};

/// A budget no page can meet: it has run out before the first content operator is looked at.
const EXPIRED: Duration = Duration::ZERO;

/// A budget well above both the default and any ordinary page, so the tests that expect a drawing
/// check the limit and not the clock. Kept clear of [`PAGE_TIME_LIMIT`] on purpose: a "generous"
/// budget equal to the default would stop distinguishing the two.
const GENEROUS: Duration = Duration::from_secs(600);

fn renderer(bytes: &[u8], page_time_limit: Duration) -> PageRenderer {
    PageRenderer::new(Arc::new(bytes.to_vec()), RenderConfig { page_time_limit, ..Default::default() })
}

fn pixels(page: usize) -> RenderRequest {
    RenderRequest { page, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() }
}

fn text(page: usize) -> RenderRequest {
    RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() }
}

/// Pages start out bounded at the sixty-second job budget, which is how one change covers export,
/// OCR, print as image, compare, word count and the previews instead of each remembering to ask.
/// The viewer is not left waiting a minute for that: `the_viewer_keeps_its_own_ceiling…` pins it.
#[test]
fn a_document_gets_a_page_budget_without_asking() {
    assert_eq!(RenderConfig::default().page_time_limit, PAGE_TIME_LIMIT);
    let out = PageRenderer::new(Arc::new(ONE_PAGE.to_vec()), RenderConfig::default()).render(pixels(0));
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.width > 0 && out.height > 0);
}

/// Turning the limit off is still available, and means what it says.
#[test]
fn a_page_can_be_given_no_limit_at_all() {
    let out = renderer(ONE_PAGE, NO_PAGE_TIME_LIMIT).render(pixels(0));
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.width > 0 && out.height > 0, "the page came back empty although nothing bounded it");
}

#[test]
fn a_page_that_finishes_inside_its_budget_is_drawn_as_usual() {
    let out = renderer(ONE_PAGE, GENEROUS).render(pixels(0));
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.width > 0 && out.height > 0, "the page came back empty although it made its budget");
}

#[test]
fn a_page_that_runs_out_of_its_budget_comes_back_skipped_with_the_watchdogs_wording() {
    let out = renderer(ONE_PAGE, EXPIRED).render(pixels(0));
    let error = out.error.unwrap_or_else(|| "the page was drawn: nothing bounded it".into());
    assert!(error.starts_with("page 1 "), "{error}");
    assert!(error.contains("took longer than"), "{error}");
    assert!(error.contains("was skipped"), "{error}");
    assert_eq!((out.width, out.height), (0, 0));
    assert!(out.rgba.is_empty(), "what a cancelled render drew is incomplete, so it is not handed over");
}

/// Text extraction is what OCR reads with, so it gets the same bound and the wording the pool uses
/// for its two kinds of request.
#[test]
fn text_extraction_runs_out_of_its_budget_too() {
    let out = renderer(ONE_PAGE, EXPIRED).render(text(0));
    assert!(out.text.is_none(), "no partial text layer");
    let error = out.error.unwrap_or_else(|| "the page was read: nothing bounded it".into());
    assert!(error.starts_with("text extraction for page 1 "), "{error}");

    let ok = renderer(ONE_PAGE, GENEROUS).render(text(0));
    assert_eq!(ok.error, None, "{:?}", ok.error);
}

/// Word count reads text outside the request path; it must not be the page read that can run for as
/// long as it likes. A tight limit must also leave the renderer usable afterwards, since a word
/// count reads every page of a document with one renderer.
#[test]
fn filtered_text_reports_the_same_limit_and_recovers_after_it() {
    let e = renderer(ONE_PAGE, EXPIRED)
        .filtered_text(0, crate::text::TextFilter::default())
        .err()
        .unwrap_or_else(|| "the page was read: nothing bounded it".into());
    assert!(e.starts_with("text extraction for page 1 "), "{e}");
    assert!(e.contains("took longer than"), "{e}");

    let mut r = renderer(ONE_PAGE_TWICE, GENEROUS);
    assert!(r.filtered_text(0, crate::text::TextFilter::default()).is_ok());
    assert!(r.filtered_text(1, crate::text::TextFilter::default()).is_ok(), "a read after a good one still works");
}

/// Each render gets a fresh budget, so one page that ran out does not spend the timer for the
/// pages after it: an export of fifty pages skips one page, not everything past it.
#[test]
fn every_page_gets_its_own_budget() {
    let mut r = renderer(ONE_PAGE_TWICE, EXPIRED);
    for page in 0..2 {
        let out = r.render(pixels(page));
        assert!(out.error.as_deref().is_some_and(|e| e.starts_with(&format!("page {} ", page + 1))), "page {}: {:?}", page + 1, out.error);
    }

    let mut r = renderer(ONE_PAGE_TWICE, GENEROUS);
    assert_eq!(r.render(pixels(0)).error, None);
    assert_eq!(r.page_count(), 2);
}

/// A page whose content is the same full-page fill over and over: it draws one black rectangle
/// whatever its count, but takes time in proportion to how often it repaints the page. The stream
/// stays small, so what the test measures is painting rather than parsing.
fn page_of_full_page_fills(operations: usize) -> Vec<u8> {
    let body = "0 0 0 rg 0 0 600 450 re f\n".repeat(operations);
    format!(
        "%PDF-1.4\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 450] /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{body}endstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        body.len()
    )
    .into_bytes()
}

/// The point of a limit is that work stops, not only that an error arrives at the end: given five
/// milliseconds for a page that needs hundreds, it comes back skipped having painted a fraction of
/// what it holds. The margin (hundreds of milliseconds against five) is wide, so this does not
/// depend on how loaded the machine happens to be.
#[test]
fn a_page_out_of_time_stops_at_its_next_operator_instead_of_finishing() {
    let busy = page_of_full_page_fills(12_000);

    let whole = renderer(&busy, GENEROUS).render(pixels(0));
    assert_eq!(whole.error, None, "{:?}", whole.error);
    assert!(whole.rgba.contains(&0), "the fixture paints the page black");
    assert!(whole.millis > 50, "the fixture is too quick to show a limit working: {} ms", whole.millis);

    let started = Instant::now();
    let out = renderer(&busy, Duration::from_millis(5)).render(pixels(0));
    let took = started.elapsed();
    assert!(out.error.as_deref().is_some_and(|e| e.contains("took longer than")), "{:?}", out.error);
    assert!(
        out.millis > 0 && out.millis * 3 < whole.millis,
        "bounded {:?} ms against {} ms unbounded: stopping early saved nothing",
        out.millis,
        whole.millis
    );
    assert!(took < Duration::from_secs(10), "the render ran to its end anyway: {took:?}");
    assert!(out.rgba.is_empty(), "and what it had painted before stopping is not handed to the caller");
}

/// The same setting bounds the viewer's pool, so there is one word for "give up on a page"
/// instead of two that can drift apart. A trivial page proves nothing here — the pool only looks
/// at the clock when its caller polls, and a quick page can finish first — so this uses the slow
/// fixture with a budget it cannot meet, and then the same page unbounded.
#[test]
fn the_pool_takes_its_limit_from_the_document_too() {
    let busy = Arc::new(page_of_full_page_fills(6_000));

    let limited = RenderConfig { page_time_limit: Duration::from_millis(5), ..Default::default() };
    let pool = RenderPool::new(busy.clone(), 1, limited);
    pool.set_queue(vec![pixels(0)]);
    let out = super::receive_before_deadline(&pool);
    assert!(out.error.as_deref().is_some_and(|e| e.contains("took longer than")), "{:?}", out.error);

    let off = RenderConfig { page_time_limit: NO_PAGE_TIME_LIMIT, ..Default::default() };
    let out = {
        let pool = RenderPool::new(busy, 1, off);
        pool.set_queue(vec![pixels(0)]);
        super::receive_before_deadline(&pool)
    };
    assert_eq!(out.error, None, "the page the limit skipped renders when the document asks for no limit");
}

/// Text extraction has two kinds of failure, and they used to arrive as the same `None`: a page that
/// is not there, and a page whose content cannot be classified honestly (an ActualText replacement
/// spanning both an included and an excluded layer). The second one arriving as "no text here" is
/// what let a caller add words over text that was already on the page, so `extract_page` now returns
/// a reason. This pins the wording for the first kind, which is what I could build without a
/// marked-content fixture: the reason reaches the caller instead of becoming silence.
#[test]
fn a_page_that_is_not_there_says_so_rather_than_reporting_no_text() {
    let out = renderer(ONE_PAGE, GENEROUS).render(text(1));
    assert!(out.text.is_none());
    let error = out.error.unwrap_or_else(|| "nothing was reported at all".into());
    assert_eq!(error, "page 2 does not exist", "{error}");

    // And a refusal is no longer mistaken for an empty page: the message carries the extractor's
    // own reason, not a claim about what the page holds.
    let e = renderer(ONE_PAGE, GENEROUS)
        .filtered_text(1, crate::text::TextFilter::default())
        .err()
        .unwrap_or_else(|| "nothing was reported at all".into());
    assert_eq!(e, "page 2 does not exist", "{e}");
}

/// `RenderConfig::default()` is deliberately uneven — every switch off except the page budget, which
/// is on (#3634) — and `..Default::default()` hides that at every call site. Destructuring the
/// default exhaustively is how a new field gets noticed: adding one without deciding its default
/// fails to compile here, instead of quietly inheriting whatever `Default` happened to do.
#[test]
fn every_setting_in_the_default_configuration_is_named_at_once() {
    let RenderConfig { password, layers, hide_comments, reject_oversize, reading_colors, transparent, page_time_limit } = RenderConfig::default();
    assert!(password.is_none());
    assert!(layers.is_empty());
    assert!(!hide_comments && !reject_oversize && !transparent);
    assert_eq!(reading_colors, ReadingColors::default());
    assert_eq!(page_time_limit, PAGE_TIME_LIMIT, "the one default that is not off");
}

/// The document's budget and the viewer's watchdog are not the same knob: asking for more time (or
/// for none) may not leave a wedged page holding the viewer with no way to recover it. The ceiling
/// is a named decision (`pool_stuck_after`), so this pins both directions — a document may make the
/// viewer keener, never slower.
#[test]
fn the_viewer_keeps_its_own_ceiling_whatever_the_document_asks_for() {
    let tighter = RenderConfig { page_time_limit: Duration::from_millis(250), ..Default::default() };
    assert_eq!(super::pool_stuck_after(&tighter), Duration::from_millis(250));

    let roomier = RenderConfig { page_time_limit: Duration::from_secs(600), ..Default::default() };
    assert_eq!(super::pool_stuck_after(&roomier), STUCK_AFTER, "the viewer does not wait ten minutes for a page");

    let off = RenderConfig { page_time_limit: NO_PAGE_TIME_LIMIT, ..Default::default() };
    assert_eq!(super::pool_stuck_after(&off), STUCK_AFTER, "opting out belongs to the jobs, not to the window");

    // The generous default must not reach the window: a pool still gives up after twenty seconds.
    assert_eq!(super::pool_stuck_after(&RenderConfig::default()), STUCK_AFTER, "the default budget is for callers that wait, not for the viewer");
}

/// #3634 review round 2: a flag that rings after the content ran to its end is not proof of an
/// early stop - rasterization never consults it, and a late bell over a finished page used to
/// throw the drawing away. Only a witnessed stop (or a zero budget) cuts a page short, and the
/// guard is where that distinction lives.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_flag_raised_after_completion_is_not_an_early_stop() {
    let g = PageTimeLimit::arm(std::time::Duration::from_millis(40)).expect("arming with a real budget spawns the timer");
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(g.cancelled.load(std::sync::atomic::Ordering::Relaxed), "the timer should have rung");
    assert!(!g.cut_short(), "nothing ever obeyed the flag: a page nobody interrupted is not a stopped one");
}

/// A zero budget is the caller's own words that no operator may run; it cuts a page short
/// without needing any interpreter to testify - which is why an empty page gets refused too.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_zero_budget_cuts_short_before_anything_runs() {
    let g = PageTimeLimit::arm(std::time::Duration::ZERO).expect("a zero budget arms with the flag already raised");
    assert!(g.cut_short(), "zero means no work, however quickly it would have finished");
}
