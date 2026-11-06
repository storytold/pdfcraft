//! Benchmarks full-document text extraction, its persistent-cache reload, and Ctrl+F matching.
//! Run this program twice against the same PDF; the second process exercises disk persistence.
use pdfcraft_render::{PageText, PreparedSearchText, RenderConfig, RenderPool, RenderRequest, RequestKind, SearchPattern, inspect};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const QUERIES: &[&str] = &["the", "anaphylaxis", "cardiac arrest", "fracture", "bleeding", "first aid"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args_os().nth(1).ok_or("usage: text_cache_bench <pdf-path>")?);
    let bytes = Arc::new(std::fs::read(&path)?);
    let page_count = inspect(bytes.clone(), None)?.pages.len();
    let threads = std::thread::available_parallelism().map(std::num::NonZeroUsize::get).unwrap_or(4).clamp(2, 8) - 1;
    let cache_open_started = Instant::now();
    let pool = RenderPool::new_with_cache_path(bytes, threads, RenderConfig { save_search_cache: true, ..Default::default() }, Some(&path));
    let cache_open_ms = cache_open_started.elapsed().as_secs_f64() * 1000.0;
    pool.set_queue((0..page_count).map(|page| RenderRequest { page, kind: RequestKind::Text, ..RenderRequest::default() }).collect());

    let checkpoints = [1, 10, 100, 325, 650, 975, page_count];
    let mut checkpoints: Vec<_> = checkpoints.into_iter().filter(|n| *n > 0 && *n <= page_count).collect();
    checkpoints.sort_unstable();
    checkpoints.dedup();
    let started = Instant::now();
    let mut checkpoint = 0;
    let mut completed = 0;
    let mut extracted = 0;
    let mut failed = 0;
    let mut worker_time_ms = 0u64;
    let mut pages: Vec<Option<Arc<PageText>>> = vec![None; page_count];
    let mut baseline_counts = vec![0usize; QUERIES.len()];
    let mut baseline_times = vec![Duration::ZERO; QUERIES.len()];
    let mut first_match_ms = None;

    while completed < page_count {
        if let Some(result) = pool.try_recv() {
            completed += 1;
            worker_time_ms = worker_time_ms.saturating_add(u64::from(result.millis));
            if let Some(text) = result.text {
                extracted += 1;
                if first_match_ms.is_none() && !text.find_opts(QUERIES[0], false, false).is_empty() {
                    first_match_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                }
                if let Some(slot) = pages.get_mut(result.request.page) {
                    *slot = Some(text);
                }
            } else {
                failed += 1;
            }
            while checkpoints.get(checkpoint).is_some_and(|target| completed >= *target) {
                let target = checkpoints[checkpoint];
                println!("{target:4} pages ready: {:8.1} ms", started.elapsed().as_secs_f64() * 1000.0);
                checkpoint += 1;
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    let stats = pool.stats();
    let mut baseline_prepared = std::collections::HashMap::<usize, Arc<PreparedSearchText>>::new();
    for (query_index, query) in QUERIES.iter().enumerate() {
        let pattern = SearchPattern::new(query, false, false);
        for (page_index, text) in pages.iter().enumerate().filter_map(|(index, text)| text.as_ref().map(|text| (index, text))) {
            let match_started = Instant::now();
            let prepared = baseline_prepared.entry(page_index).or_insert_with(|| Arc::new(pattern.prepare_page(text))).clone();
            let matches = pattern.find_prepared(&prepared);
            baseline_times[query_index] += match_started.elapsed();
            baseline_counts[query_index] = baseline_counts[query_index].saturating_add(matches.len());
        }
    }
    println!("total: {:.1} ms; text pages: {extracted}; failed: {failed}", started.elapsed().as_secs_f64() * 1000.0);
    println!("summed worker time: {worker_time_ms} ms");
    println!("baseline SIMD matching: {:.1} ms across {} queries", baseline_times.iter().sum::<Duration>().as_secs_f64() * 1000.0, QUERIES.len());
    println!("first match: {}", first_match_ms.map_or_else(|| "none".into(), |ms| format!("{ms:.1} ms")));
    println!("text interpretations: {}; parser builds: {}", stats.text_interpretations, stats.parser_builds);
    println!("cache open and index load: {cache_open_ms:.1} ms");

    let mut indexed_time = Duration::ZERO;
    let mut indexed_prepared = std::collections::HashMap::<usize, Arc<PreparedSearchText>>::new();
    for (query_index, query) in QUERIES.iter().enumerate() {
        let pattern = SearchPattern::new(query, false, false);
        let candidates = pool.text_candidate_pages(query, page_count, false).unwrap_or_else(|| (0..page_count).collect());
        let search_started = Instant::now();
        let indexed_matches = candidates
            .iter()
            .filter_map(|page| pages.get(*page).and_then(Option::as_ref).map(|text| (*page, text)))
            .map(|(page, text)| {
                let prepared = indexed_prepared.entry(page).or_insert_with(|| Arc::new(pattern.prepare_page(text))).clone();
                pattern.find_prepared(&prepared).len()
            })
            .sum::<usize>();
        let search_time = search_started.elapsed();
        indexed_time += search_time;
        if indexed_matches != baseline_counts[query_index] {
            return Err(format!("trigram candidate index changed results for query {query:?}").into());
        }
        println!(
            "cached-index query {query:?}: {} candidate pages; {:.2} ms (SIMD all-pages {:.2} ms; same {} matches)",
            candidates.len(),
            search_time.as_secs_f64() * 1000.0,
            baseline_times[query_index].as_secs_f64() * 1000.0,
            indexed_matches
        );
    }
    println!(
        "cached-index queries: {:.1} ms total (SIMD baseline {:.1} ms)",
        indexed_time.as_secs_f64() * 1000.0,
        baseline_times.iter().sum::<Duration>().as_secs_f64() * 1000.0
    );
    Ok(())
}
