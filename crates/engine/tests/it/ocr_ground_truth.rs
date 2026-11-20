//! Every OCR ground-truth text (`tests/data/ocr/`), set in Inter and scanned at 200 dpi with noise,
//! blur and JPEG compression, reads back at least as well as it did when its floor was recorded.
//! The floors are the measured share minus a small margin, so a regression in the recogniser,
//! the render or the text layer fails while run-to-run noise does not.
//!
//! Each fixture is read with the recogniser of its own language when PdfCraft has one
//! ([`ocr_scan::recogniser_for`]), otherwise with English (ocrs: the Latin alphabet without
//! accents). Floors are kept per (fixture, recogniser): when a recogniser for German lands
//! (#1012, #2520), `de.tsv` is read with it and `every_fixture_has_a_floor_and_inter_draws_it`
//! asks for that floor to be measured and added, while the English rows document what ocrs reads.
//!
//! Skipped when the models are not installed (`cargo xtask models`). The full matrix of faces and
//! scan conditions is `cargo run --release -p pdfcraft-engine --example ocr_bench -- --all`;
//! docs/ocr-benchmark.md explains the fixtures and the scores.

use pdfcraft_engine::{ImageResolution, Session, ocr};

use crate::support::ocr_scan::{self, Score};

/// (fixture file, recogniser, minimum share of all words, minimum share of words with a letter
/// outside ASCII). The English rows were measured on 2026-10-28 with ocrs on main (Inter,
/// `200noisy`, text at 11 pt), less two points; ocrs's alphabet has no accented letters, so its
/// accented floor is what it reads of them, nothing.
const FLOORS: &[(&str, &str, f64, f64)] = &[
    ("cs-wikipedia.tsv", "en", 0.35, 0.0),
    ("da-wikipedia.tsv", "en", 0.74, 0.0),
    ("de-wikipedia.tsv", "en", 0.71, 0.0),
    ("de.tsv", "en", 0.64, 0.0),
    ("es-wikipedia.tsv", "en", 0.77, 0.0),
    ("fi-wikipedia.tsv", "en", 0.62, 0.0),
    ("fr-wikipedia.tsv", "en", 0.72, 0.0),
    ("is-wikipedia.tsv", "en", 0.38, 0.0),
    ("it-wikipedia.tsv", "en", 0.79, 0.0),
    ("nl-wikipedia.tsv", "en", 0.86, 0.0),
    ("no-wikipedia.tsv", "en", 0.74, 0.0),
    ("pl-wikipedia.tsv", "en", 0.46, 0.0),
    ("pt-wikipedia.tsv", "en", 0.73, 0.0),
    ("sv-wikipedia.tsv", "en", 0.52, 0.0),
];

fn floor(file: &str, recogniser: &str) -> Option<(f64, f64)> {
    FLOORS.iter().find(|(f, r, ..)| *f == file && *r == recogniser).map(|&(_, _, words, accented)| (words, accented))
}

#[test]
fn every_fixture_has_a_floor_and_inter_draws_it() {
    let faces = ocr_scan::builtin_faces();
    let fixtures = ocr_scan::all().unwrap();
    assert!(!fixtures.is_empty(), "no fixture in {}", ocr_scan::data_dir().display());
    for f in &fixtures {
        let recogniser = ocr_scan::recogniser_for(&f.lang);
        assert!(floor(&f.file, recogniser).is_some(), "{}: measure it with the {recogniser} recogniser and add a floor to FLOORS", f.file);
        for doc in &f.docs {
            assert_eq!(ocr_scan::missing_glyphs(&faces[0], &doc.text()), Vec::<char>::new(), "{} {}", f.file, doc.name);
        }
    }
    for (file, recogniser, ..) in FLOORS {
        assert!(fixtures.iter().any(|f| f.file == *file), "{file}: a floor for a fixture that does not exist");
        assert!(ocr_scan::ocr_language_known(recogniser), "{file}: {recogniser} is not an OCR language");
    }
}

#[test]
fn scanned_ground_truth_reads_back_above_its_floor() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed (cargo xtask models, or set PDFCRAFT_MODELS)");
        return;
    }
    let face = &ocr_scan::builtin_faces()[0];
    let cond = ocr_scan::condition("200noisy").unwrap();
    let mut failures = Vec::new();
    for f in ocr_scan::all().unwrap() {
        let recogniser = ocr_scan::recogniser_for(&f.lang);
        let mut total = Score::default();
        for doc in &f.docs {
            let image = ocr_scan::scan(doc, face, cond, ocr_scan::seed(&[&f.file, &doc.name, &face.name, cond.name])).unwrap();
            let mut s = Session::new();
            let pdf = s.create_from_images_with_resolution(&[("scan.jpg".into(), image)], ImageResolution::Dpi(f64::from(cond.dpi))).unwrap();
            let id = s.open("scan.pdf", None, pdf, None).unwrap();
            let settings = ocr::OcrSettings { dpi: cond.dpi as f32, language: recogniser.into(), ..Default::default() };
            let text: Vec<String> = s.recognize_text(id, &[], settings).unwrap().iter().map(|p| p.text()).collect();
            total.add(ocr_scan::score(&doc.text(), &text.join("\n")));
        }
        eprintln!("{} ({recogniser}): words {:.3}, accented {:.3}", f.file, total.words_rate(), total.accented_rate());
        let Some((words, accented)) = floor(&f.file, recogniser) else { continue };
        if total.words_rate() < words || total.accented_rate() < accented {
            failures.push(format!(
                "{} ({recogniser}): words {:.3} (floor {words}), accented {:.3} (floor {accented})",
                f.file,
                total.words_rate(),
                total.accented_rate()
            ));
        }
    }
    assert!(failures.is_empty(), "OCR fell below its floor:\n{}", failures.join("\n"));
}
