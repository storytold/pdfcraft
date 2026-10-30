//! OCR benchmark on real pages: MDPBench's public split (Apache-2.0; digital pages and phone
//! photos of them with folds, shadows and blur), fetched by `cargo xtask mdpbench`. Each page is
//! recognised in each OCR language and scored word by word against the text of its annotated text
//! regions, as `ocr_bench` scores the synthetic fixtures.
//!
//! ```sh
//! cargo xtask mdpbench --langs de,fr                      # once; pages go to target/mdpbench/
//! cargo run --release -p pdfcraft-engine --example ocr_mdpbench -- target/mdpbench --langs de,fr --out predictions
//! ```
//!
//! `--langs` picks MDPBench's languages (file prefixes), `--ocr` the recognition languages to
//! compare (default: `en` and the page's own language, when PdfCraft has a recogniser for it; one
//! it has none for is skipped with a message), `--kind digital`,
//! `photo` (taken from above) or `photo-turned` (sideways or upside down) one part, `--limit N`
//! the first N pages per language and kind, `--misses` the words that did not come back.
//! `--out DIR` writes each page's text to `DIR/<OCR language>/<image>.md`, the predictions
//! MDPBench's own evaluation reads (`pdf_validation.py`, then `tools/calculate_scores.py`); its
//! scores are the ones published. A page that fails gets an empty prediction (the evaluation
//! skips pages without one), and a prediction that cannot be written stops the run.
//!
//! The scores printed here are a quick check of our own: words found, in three classes, against
//! the text of the annotated regions. Table cells (from the annotated HTML) are scored on their
//! own, so misread figures in tables show; formulas, figures and regions the annotators set aside
//! are not scored, as recognised words cannot be compared with LaTeX. docs/ocr-benchmark.md has the
//! whole procedure and the current numbers.

#[path = "../tests/it/support/ocr_scan.rs"]
mod ocr_scan;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ocr_scan::Score;
use pdfcraft_engine::{ImageResolution, Session, ocr};

/// The region categories whose text is scored.
const TEXT: &[&str] = &[
    "text_block",
    "title",
    "header",
    "footer",
    "page_number",
    "figure_caption",
    "figure_footnote",
    "table_caption",
    "table_footnote",
    "page_footnote",
    "equation_caption",
    "equation_explanation",
];
/// A page is read at the resolution that makes it this wide (A4), within these bounds.
const PAGE_WIDTH_INCHES: f64 = 8.27;
const DPI: (f64, f64) = (96.0, 600.0);

struct Args {
    dir: PathBuf,
    langs: Vec<String>,
    ocr: Vec<String>,
    kind: Option<&'static str>,
    limit: usize,
    misses: bool,
    /// Where to write each page's text as Markdown for MDPBench's own evaluation.
    out: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args { dir: PathBuf::new(), langs: vec![], ocr: vec![], kind: None, limit: usize::MAX, misses: false, out: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--langs" => a.langs = value()?.split(',').map(str::to_string).collect(),
            "--ocr" => a.ocr = value()?.split(',').map(str::to_string).collect(),
            "--kind" => {
                a.kind = Some(match value()?.as_str() {
                    "digital" => "digital",
                    "photo" => "photo",
                    "photo-turned" => "photo-turned",
                    k => return Err(format!("--kind is digital, photo or photo-turned, not {k}")),
                })
            }
            "--limit" => a.limit = value()?.parse().map_err(|_| "--limit needs a number".to_string())?,
            "--misses" => a.misses = true,
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => a.dir = PathBuf::from(s),
        }
    }
    if a.dir.as_os_str().is_empty() {
        return Err("name the directory `cargo xtask mdpbench` filled (target/mdpbench)".into());
    }
    Ok(a)
}

/// A page of the benchmark: its file, language prefix, kind and the text it should give.
struct Page {
    file: String,
    lang: String,
    kind: &'static str,
    truth: String,
    /// The text of the page's table cells.
    tables: String,
}

/// The text of an HTML table's cells, one cell per line: tags dropped, the few entities tables use
/// decoded.
fn cells(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                tag = true;
                out.push(' ');
            }
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&quot;", "\"")
}

/// The photos of a digital original name how they were taken (lang_type_n_indoor_light_top_…):
/// taken from the left, the right or below, the text lies sideways or upside down, which
/// recognition without turning the page cannot read; those are counted apart.
fn kind(file: &str) -> &'static str {
    if !file.contains("_indoor") && !file.contains("_outdoor") {
        return "digital";
    }
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s).replace(' ', "");
    if stem.split('_').any(|t| matches!(t, "left" | "right" | "down")) { "photo-turned" } else { "photo" }
}

/// A region's reading order (a number, or a number in a string).
fn order(r: &serde_json::Value) -> i64 {
    match r.get("order") {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(i64::MAX),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(i64::MAX),
        _ => i64::MAX,
    }
}

fn pages(dir: &Path) -> Result<Vec<Page>, String> {
    let path = dir.join("MDPBench_public.json");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e} (run cargo xtask mdpbench)", path.display()))?;
    let all: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for p in all.as_array().ok_or("the annotations are not a list")? {
        let Some(file) = p.pointer("/page_info/image_path").and_then(|v| v.as_str()) else { continue };
        // A name with a directory part would be read from outside `DIR/images` and written
        // outside `--out`; MDPBench names its images plainly, so such a page is not one of its own.
        let Some(lang) = ocr_scan::mdpbench_language(file) else {
            eprintln!("skipped {file:?}: not a plain image file name");
            continue;
        };
        let kind = kind(file);
        let regions: Vec<&serde_json::Value> = p.get("layout_dets").and_then(|v| v.as_array()).into_iter().flatten().collect();
        let category = |r: &serde_json::Value| r.get("category_type").and_then(|c| c.as_str()).unwrap_or_default().to_string();
        let mut text: Vec<(i64, &str)> =
            regions.iter().filter(|r| TEXT.contains(&category(r).as_str())).filter_map(|r| Some((order(r), r.get("text")?.as_str()?))).collect();
        text.sort_by_key(|r| r.0);
        let truth = text.iter().map(|r| r.1).collect::<Vec<_>>().join("\n");
        let tables =
            regions.iter().filter(|r| category(r) == "table").filter_map(|r| r.get("html")?.as_str()).map(cells).collect::<Vec<_>>().join("\n");
        if !truth.trim().is_empty() || !tables.trim().is_empty() {
            out.push(Page { file: file.to_string(), lang: lang.to_string(), kind, truth, tables });
        }
    }
    Ok(out)
}

fn recognise(image: Vec<u8>, language: &str) -> Result<String, String> {
    let width = image::load_from_memory(&image).map_err(|e| e.to_string())?.width();
    let dpi = (f64::from(width) / PAGE_WIDTH_INCHES).clamp(DPI.0, DPI.1);
    let mut s = Session::new();
    let pdf = s.create_from_images_with_resolution(&[("page.jpg".into(), image)], ImageResolution::Dpi(dpi)).map_err(|e| e.to_string())?;
    let id = s.open("page.pdf", None, pdf, None).map_err(|e| format!("{e:?}"))?;
    let settings = ocr::OcrSettings { dpi: dpi as f32, language: language.into(), ..Default::default() };
    let pages = s.recognize_text(id, &[], settings)?;
    // A page the recogniser gave up on is an error here, not an empty page.
    if let Some(why) = pages.iter().find_map(|p| p.skipped.as_deref()) {
        return Err(format!("page skipped: {why}"));
    }
    Ok(pages.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n"))
}

#[derive(Default)]
struct Cell {
    score: Score,
    tables: Score,
    pages: usize,
    seconds: f64,
    failed: usize,
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "{e}\n\nusage: ocr_mdpbench DIR [--langs de,fr] [--ocr en,de] [--kind digital|photo|photo-turned] [--limit N] [--misses] [--out DIR]"
            );
            std::process::exit(2);
        }
    };
    if !ocr::available() {
        eprintln!("the OCR models are not installed: run cargo xtask models, or set PDFCRAFT_MODELS");
        std::process::exit(2);
    }
    let known: Vec<&str> = ocr::LANGUAGES.iter().map(|(code, _)| *code).collect();
    for l in args.ocr.iter().filter(|l| !ocr_scan::ocr_language_known(l)) {
        eprintln!("skipped {l}: PdfCraft has no recogniser for it (it has {})", known.join(", "));
    }
    if !args.ocr.is_empty() && args.ocr.iter().all(|l| !ocr_scan::ocr_language_known(l)) {
        std::process::exit(2);
    }
    let all = match pages(&args.dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let mut taken: BTreeMap<(String, &str), usize> = BTreeMap::new();
    let chosen: Vec<&Page> = all
        .iter()
        .filter(|p| args.langs.is_empty() || args.langs.contains(&p.lang))
        .filter(|p| args.kind.is_none_or(|k| k == p.kind))
        .filter(|p| args.dir.join("images").join(&p.file).is_file())
        .filter(|p| {
            let n = taken.entry((p.lang.clone(), p.kind)).or_default();
            *n += 1;
            *n <= args.limit
        })
        .collect();
    if chosen.is_empty() {
        eprintln!("no fetched pages match; run cargo xtask mdpbench --langs {}", args.langs.join(","));
        std::process::exit(2);
    }
    let mut cells: BTreeMap<(String, String, &str), Cell> = BTreeMap::new();
    let mut misses: BTreeMap<(String, String), BTreeMap<String, usize>> = BTreeMap::new();
    let mut ocr_langs: Vec<String> = if args.ocr.is_empty() { vec!["en".to_string()] } else { args.ocr.clone() };
    ocr_langs.retain(|l| ocr_scan::ocr_language_known(l));
    let ocr_langs: Vec<String> = ocr_langs.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    let mut failed = 0usize;
    for page in &chosen {
        let mut ocr_langs = ocr_langs.clone();
        if args.ocr.is_empty() && ocr_scan::ocr_language_known(&page.lang) && !ocr_langs.contains(&page.lang) {
            ocr_langs.push(page.lang.clone());
        }
        // A page that cannot be read counts as failed for every recogniser, like one the
        // recogniser gives up on, rather than dropping out of the run.
        let image = std::fs::read(args.dir.join("images").join(&page.file)).map_err(|e| e.to_string());
        for language in &ocr_langs {
            let started = Instant::now();
            let result = image.clone().and_then(|image| recognise(image, language));
            let cell = cells.entry((page.lang.clone(), language.clone(), page.kind)).or_default();
            cell.seconds += started.elapsed().as_secs_f64();
            cell.pages += 1;
            // MDPBench's evaluation skips a page without a prediction, so a failed page gets an
            // empty one, and a prediction that cannot be written ends the run.
            if let Some(out) = &args.out
                && let Err(e) = ocr_scan::write_prediction(out, language, &page.file, result.as_deref().ok())
            {
                eprintln!("{} {language}: cannot write the prediction under {}: {e}", page.file, out.display());
                std::process::exit(1);
            }
            match result {
                Ok(text) => {
                    cell.score.add(ocr_scan::score(&page.truth, &text));
                    cell.tables.add(ocr_scan::score(&page.tables, &text));
                    let mut have: BTreeMap<String, usize> = BTreeMap::new();
                    for t in ocr_scan::tokens(&text) {
                        *have.entry(t).or_default() += 1;
                    }
                    let lost = misses.entry((page.lang.clone(), language.clone())).or_default();
                    for t in ocr_scan::tokens(&page.truth) {
                        match have.get_mut(&t) {
                            Some(n) if *n > 0 => *n -= 1,
                            _ => *lost.entry(t).or_default() += 1,
                        }
                    }
                }
                Err(e) => {
                    cell.failed += 1;
                    failed += 1;
                    eprintln!("{} {language}: {e}", page.file);
                }
            }
        }
    }
    println!("| Language | OCR | Pages | Kind | Words | Accented words | Numbers & symbols | Table cells: words | s/page |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for ((lang, ocr_lang, kind), c) in &cells {
        let failed = if c.failed > 0 { format!(" ({} failed)", c.failed) } else { String::new() };
        // The shares are over the pages that were read; with none read there is no share.
        let read = c.failed < c.pages;
        let pct = |r: f64| if read { format!("{:.1} %", r * 100.0) } else { "–".to_string() };
        println!(
            "| {lang} | {ocr_lang} | {}{failed} | {kind} | {} | {} | {} | {} | {:.2} |",
            c.pages,
            pct(c.score.words_rate()),
            pct(c.score.accented_rate()),
            pct(c.score.symbols_rate()),
            pct(c.tables.words_rate()),
            c.seconds / c.pages.max(1) as f64
        );
    }
    if args.misses {
        for ((lang, ocr_lang), lost) in &misses {
            let mut v: Vec<(&String, &usize)> = lost.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list: Vec<String> = v.iter().take(40).map(|(w, n)| format!("{w} ×{n}")).collect();
            println!("\nMissed in {lang} with {ocr_lang}: {}", list.join(", "));
        }
    }
    if failed > 0 && args.out.is_some() {
        eprintln!("{failed} page readings failed; their predictions are empty, so the official evaluation scores them as nothing found");
    }
}
