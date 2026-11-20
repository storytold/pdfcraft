//! OCR benchmark over the ground-truth texts in `tests/data/ocr/`: each document is set in each
//! face, scanned under each condition, recognised in each OCR language, and scored word by word.
//!
//! ```sh
//! cargo xtask models                                      # once
//! cargo run --release -p pdfcraft-engine --example ocr_bench -- de
//! cargo run --release -p pdfcraft-engine --example ocr_bench -- --all --cond 200noisy --face ~/fonts/CharisSIL-Regular.ttf
//! ```
//!
//! Positional arguments pick fixtures by language (`de` takes `de.tsv` and `de-wikipedia.tsv`);
//! `--all` takes every file. `--ocr` lists the recognition languages to compare (default: `en`
//! and the fixture's own language); a language PdfCraft has no recogniser for (`ocr::LANGUAGES`)
//! is skipped with a message. `--cond` limits the scan conditions, `--face` adds a font file to
//! the two committed faces, `--faces Inter,…` keeps only the named ones, `--save DIR` keeps every
//! scan, `--misses` lists the words that did not come back. docs/ocr-benchmark.md explains the
//! fixtures, the conditions and the scores.

#[path = "../tests/it/support/ocr_scan.rs"]
mod ocr_scan;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use ocr_scan::{CONDITIONS, Condition, Face, Fixture, Score};
use pdfcraft_engine::{ImageResolution, Session, ocr};

struct Args {
    langs: Vec<String>,
    all: bool,
    ocr: Vec<String>,
    conds: Vec<Condition>,
    faces: Vec<Face>,
    save: Option<PathBuf>,
    misses: bool,
}

fn parse() -> Result<Args, String> {
    let mut a =
        Args { langs: vec![], all: false, ocr: vec![], conds: CONDITIONS.to_vec(), faces: ocr_scan::builtin_faces(), save: None, misses: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--all" => a.all = true,
            "--ocr" => a.ocr = value()?.split(',').map(str::to_string).collect(),
            "--cond" => {
                let names = value()?;
                a.conds = names.split(',').map(|n| ocr_scan::condition(n).ok_or(format!("unknown condition {n}"))).collect::<Result<_, _>>()?;
            }
            "--face" => {
                let path = PathBuf::from(value()?);
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("face").to_string();
                a.faces.push(Face { name, bytes });
            }
            "--faces" => {
                let keep: Vec<String> = value()?.split(',').map(str::to_string).collect();
                a.faces.retain(|f| keep.contains(&f.name));
            }
            "--save" => a.save = Some(PathBuf::from(value()?)),
            "--misses" => a.misses = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => a.langs.push(s.to_string()),
        }
    }
    if a.langs.is_empty() && !a.all {
        return Err("name a fixture language (de, fr, …) or --all".into());
    }
    Ok(a)
}

/// Totals per (fixture, OCR language, condition), and the words that did not come back.
#[derive(Default)]
struct Cell {
    score: Score,
    pages: usize,
    seconds: f64,
    failed: usize,
}

fn recognise(image: Vec<u8>, cond: Condition, language: &str) -> Result<String, String> {
    let mut s = Session::new();
    let pdf =
        s.create_from_images_with_resolution(&[("scan.jpg".into(), image)], ImageResolution::Dpi(f64::from(cond.dpi))).map_err(|e| e.to_string())?;
    let id = s.open("scan.pdf", None, pdf, None).map_err(|e| format!("{e:?}"))?;
    let settings = ocr::OcrSettings { dpi: cond.dpi as f32, language: language.into(), ..Default::default() };
    let pages = s.recognize_text(id, &[], settings)?;
    Ok(pages.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n"))
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\nusage: ocr_bench <lang>… | --all [--ocr en,de] [--cond 300clean,…] [--face FILE] [--save DIR] [--misses]");
            std::process::exit(2);
        }
    };
    if !ocr::available() {
        eprintln!("the OCR models are not installed: run cargo xtask models, or set PDFCRAFT_MODELS");
        std::process::exit(2);
    }
    let fixtures: Vec<Fixture> = match ocr_scan::all() {
        Ok(all) => all.into_iter().filter(|f| args.all || args.langs.contains(&f.lang)).collect(),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if fixtures.is_empty() {
        eprintln!("no fixture in {} matches", ocr_scan::data_dir().display());
        std::process::exit(2);
    }
    let mut cells: BTreeMap<(String, String, &str), Cell> = BTreeMap::new();
    let mut misses: BTreeMap<(String, String), BTreeMap<String, usize>> = BTreeMap::new();
    let mut extras: BTreeMap<(String, String), BTreeMap<String, usize>> = BTreeMap::new();
    let mut unknown = BTreeSet::new();
    for fixture in &fixtures {
        let mut ocr_langs = if args.ocr.is_empty() { vec!["en".to_string(), fixture.lang.clone()] } else { args.ocr.clone() };
        ocr_langs.sort();
        ocr_langs.dedup();
        for language in ocr_langs.iter() {
            if !ocr_scan::ocr_language_known(language) {
                if unknown.insert(language.clone()) {
                    let known: Vec<&str> = ocr::LANGUAGES.iter().map(|(code, _)| *code).collect();
                    eprintln!("skipped {language}: PdfCraft has no recogniser for it (it has {})", known.join(", "));
                }
                continue;
            }
            for doc in &fixture.docs {
                for face in &args.faces {
                    for &cond in &args.conds {
                        let seed = ocr_scan::seed(&[&fixture.file, &doc.name, &face.name, cond.name]);
                        let image = match ocr_scan::scan(doc, face, cond, seed) {
                            Ok(i) => i,
                            Err(e) => {
                                eprintln!("{} {}: {e}", fixture.file, doc.name);
                                continue;
                            }
                        };
                        if let Some(dir) = &args.save {
                            let ext = if cond.jpeg.is_some() { "jpg" } else { "png" };
                            let _ = std::fs::create_dir_all(dir);
                            let _ = std::fs::write(dir.join(format!("{}_{}_{}_{}.{ext}", fixture.lang, doc.name, face.name, cond.name)), &image);
                        }
                        let started = Instant::now();
                        let result = recognise(image, cond, language);
                        let cell = cells.entry((fixture.file.clone(), language.clone(), cond.name)).or_default();
                        cell.seconds += started.elapsed().as_secs_f64();
                        cell.pages += 1;
                        match result {
                            Ok(text) => {
                                cell.score.add(ocr_scan::score(&doc.text(), &text));
                                let got: Vec<String> = ocr_scan::tokens(&text);
                                let lost = misses.entry((fixture.file.clone(), language.clone())).or_default();
                                let mut have: BTreeMap<&str, usize> = BTreeMap::new();
                                for t in &got {
                                    *have.entry(t).or_default() += 1;
                                }
                                for t in ocr_scan::tokens(&doc.text()) {
                                    match have.get_mut(t.as_str()) {
                                        Some(n) if *n > 0 => *n -= 1,
                                        _ => *lost.entry(t).or_default() += 1,
                                    }
                                }
                                let extra = extras.entry((fixture.file.clone(), language.clone())).or_default();
                                for (t, n) in have.into_iter().filter(|(_, n)| *n > 0) {
                                    *extra.entry(t.to_string()).or_default() += n;
                                }
                            }
                            Err(e) => {
                                cell.failed += 1;
                                eprintln!("{} {} {} {} {language}: {e}", fixture.file, doc.name, face.name, cond.name);
                            }
                        }
                    }
                }
            }
        }
    }
    println!("| Fixture | OCR | Scan | Pages | Words | Accented words | Numbers & symbols | s/page |");
    println!("|---|---|---|---|---|---|---|---|");
    let pct = |r: f64| format!("{:.1} %", r * 100.0);
    let mut totals: BTreeMap<(String, String), Cell> = BTreeMap::new();
    for ((file, lang, cond), c) in &cells {
        let failed = if c.failed > 0 { format!(" ({} failed)", c.failed) } else { String::new() };
        println!(
            "| {file} | {lang} | {cond} | {}{failed} | {} | {} | {} | {:.2} |",
            c.pages,
            pct(c.score.words_rate()),
            pct(c.score.accented_rate()),
            pct(c.score.symbols_rate()),
            c.seconds / c.pages.max(1) as f64
        );
        let t = totals.entry((file.clone(), lang.clone())).or_default();
        t.score.add(c.score);
        t.pages += c.pages;
        t.seconds += c.seconds;
        t.failed += c.failed;
    }
    for ((file, lang), c) in &totals {
        println!(
            "| **{file}** | **{lang}** | **all** | {} | **{}** | **{}** | **{}** | {:.2} |",
            c.pages,
            pct(c.score.words_rate()),
            pct(c.score.accented_rate()),
            pct(c.score.symbols_rate()),
            c.seconds / c.pages.max(1) as f64
        );
    }
    if args.misses {
        for ((file, lang), lost) in &misses {
            let mut v: Vec<(&String, &usize)> = lost.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list: Vec<String> = v.iter().take(40).map(|(w, n)| format!("{w} ×{n}")).collect();
            println!("\nMissed in {file} with {lang}: {}", list.join(", "));
            if let Some(extra) = extras.get(&(file.clone(), lang.clone())) {
                let mut v: Vec<(&String, &usize)> = extra.iter().collect();
                v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                let list: Vec<String> = v.iter().take(40).map(|(w, n)| format!("{w} ×{n}")).collect();
                println!("Read instead: {}", list.join(", "));
            }
        }
    }
}
