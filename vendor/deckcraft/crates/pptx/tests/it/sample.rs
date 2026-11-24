//! The engine's sample deck: export, re-import, compare. Set `DECKCRAFT_PPTX_OUT=<dir>` to also
//! write the exported file there (for checking it in other applications).

use deckcraft_engine::Session;
use deckcraft_model::{Presentation, ShapeKind};

fn sample() -> Presentation {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    s.doc().expect("doc").doc.as_ref().clone()
}

fn same_shapes(a: &deckcraft_model::Slide, b: &deckcraft_model::Slide) {
    assert_eq!(a.shapes.len(), b.shapes.len(), "shape count on slide '{}'", a.title());
    for (sa, sb) in a.shapes.iter().zip(b.shapes.iter()) {
        assert_eq!(sa.kind_name(), sb.kind_name(), "{}", sa.name);
        assert_eq!(sa.text.as_ref().map(|t| t.text()), sb.text.as_ref().map(|t| t.text()), "{}", sa.name);
        if let (Some(x), Some(y)) = (sa.xfrm, sb.xfrm) {
            assert!((x.x - y.x).abs() < 0.01 && (x.w - y.w).abs() < 0.01, "{}: {x:?} vs {y:?}", sa.name);
        }
        if let (ShapeKind::Table(ta), ShapeKind::Table(tb)) = (&sa.kind, &sb.kind) {
            assert_eq!(ta.style, tb.style);
            assert_eq!(ta.rows.len(), tb.rows.len());
        }
        if let (ShapeKind::Chart(ca), ShapeKind::Chart(cb)) = (&sa.kind, &sb.kind) {
            assert_eq!(ca.kind, cb.kind);
            assert_eq!(ca.categories, cb.categories);
            assert_eq!(ca.series.len(), cb.series.len());
            for (x, y) in ca.series.iter().zip(cb.series.iter()) {
                assert_eq!(x.name, y.name);
                assert_eq!(x.values, y.values);
            }
        }
    }
}

#[test]
fn sample_deck_round_trips() {
    let p = sample();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    if let Ok(dir) = std::env::var("DECKCRAFT_PPTX_OUT") {
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(format!("{dir}/sample.pptx"), &bytes).expect("write");
    }
    assert!(deckcraft_pptx::sniff(&bytes));
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(q.slides.len(), p.slides.len());
    assert!(q.validate().is_empty(), "{:?}", q.validate());
    for (a, b) in p.slides.iter().zip(q.slides.iter()) {
        assert_eq!(a.title(), b.title());
        assert_eq!(a.notes_text(), b.notes_text());
        same_shapes(a, b);
        assert_eq!(a.transition.as_ref().map(|t| (&t.kind, &t.option)), b.transition.as_ref().map(|t| (&t.kind, &t.option)));
        assert_eq!(a.animations.len(), b.animations.len(), "animations on '{}'", a.title());
        for (x, y) in a.animations.iter().zip(b.animations.iter()) {
            assert_eq!(
                (&x.effect, x.class, x.start, x.duration_ms, x.delay_ms, x.text_build),
                (&y.effect, y.class, y.start, y.duration_ms, y.delay_ms, y.text_build)
            );
            assert_eq!(x.shape, y.shape);
        }
    }
    assert_eq!(q.sections.len(), p.sections.len());
    for (a, b) in p.sections.iter().zip(q.sections.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.slides.len(), b.slides.len());
    }
    // A second round trip is stable.
    let bytes2 = deckcraft_pptx::export(&q).expect("export 2");
    let r = deckcraft_pptx::import(&bytes2).expect("import 2");
    assert_eq!(r.slides.len(), q.slides.len());
}

/// Reads any file named by `DECKCRAFT_PPTX_IN` (for checking decks made by other applications).
#[test]
fn external_file_imports() {
    let Ok(path) = std::env::var("DECKCRAFT_PPTX_IN") else { return };
    let bytes = std::fs::read(&path).expect("read");
    let p = deckcraft_pptx::import(&bytes).expect("import");
    println!("{path}: {} slides, {} masters, {} layouts", p.slides.len(), p.masters.len(), p.masters.iter().map(|m| m.layouts.len()).sum::<usize>());
    for (i, s) in p.slides.iter().enumerate() {
        let kinds: Vec<&str> = s.shapes.iter().map(|s| s.kind_name()).collect();
        println!("  slide {}: title {:?} shapes {:?} layout {:?}", i + 1, s.title(), kinds, p.layout(s.layout).map(|(_, l)| l.name.clone()));
    }
    assert!(p.validate().is_empty());
    let again = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("reimport");
    assert_eq!(again.slides.len(), p.slides.len());
    if let Ok(dir) = std::env::var("DECKCRAFT_PPTX_OUT") {
        std::fs::write(format!("{dir}/reexport.pptx"), deckcraft_pptx::export(&p).expect("export")).expect("write");
    }
}
