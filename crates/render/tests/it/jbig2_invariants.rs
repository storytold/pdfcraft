//! Every JBIG2 conformance fixture in the corpus encodes the same picture.
//!
//! The 96 `bitmap-*.pdf` files are a conformance suite: one small drawing, encoded through every
//! corner of JBIG2 — generic regions with each template and with custom AT pixels, typical
//! prediction (TPGDON, TPGRON), refinement, halftone regions with and without skip, symbol
//! dictionaries with Huffman tables, MMR, composition operators, random access, unknown-height
//! segments. The features change how the bytes are compressed. They do not change the picture.
//!
//! So a correct decoder renders all 96 identically, and any file that comes out different is a
//! decoder bug. That is a real test with no reference image and no external oracle: the suite
//! checks itself.
//!
//! It is worth having because the property settled a question the render oracle raised and could
//! not answer. `cargo xtask render-oracle` put four JBIG2 fixtures at the top of its disagreement
//! list against Poppler, and the natural reading was that `hayro-jbig2`, which carries local
//! patches, had a bug. The opposite is true. Measured over this suite: **we render 1 distinct
//! image across all 96; Poppler renders 8**, drawing nothing at all for the seven halftone-skip
//! and symbol fixtures and mis-decoding TPGRON refinement. An oracle is not ground truth. This
//! invariant is.
//!
//! Runs over `corpus/pdfjs/test/pdfs` when present (`cargo xtask corpus` fetches and verifies it);
//! a no-op when absent, so CI without the corpus stays green. The expectation that every fixture
//! holds the same picture is a property of the **pinned** corpus; whoever re-pins it to a newer
//! pdf.js should re-check this test rather than assume it still holds.

use std::collections::BTreeMap;
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// `corpus/pdfjs/test/pdfs`, or `None` when nothing has been fetched.
///
/// Panics when the checkout exists without the stamp `cargo xtask corpus` writes on a successful
/// verification: an unverified corpus must fail loudly rather than skip, as in `pdfcraft-cos`'s
/// corpus tests.
fn corpus() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pdfjs");
    if !dir.is_dir() {
        return None;
    }
    assert!(
        dir.join(".pdfcraft-corpus.json").is_file(),
        "{} exists but has not been verified against the pin; run `cargo xtask corpus`",
        dir.display()
    );
    Some(dir.join("test/pdfs"))
}

/// Page 1 as premultiplied RGBA at 1:1, or `None` when it could not be rendered.
fn render(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig { reject_oversize: true, ..Default::default() });
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    page.error.is_none().then_some((page.width, page.height, page.rgba.to_vec()))
}

#[test]
#[ignore = "needs corpus/; run with --ignored"]
fn every_jbig2_conformance_fixture_decodes_to_the_same_picture() {
    let Some(dir) = corpus() else {
        eprintln!("corpus not fetched; skipping (cargo xtask corpus)");
        return;
    };
    let mut fixtures: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf") && p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("bitmap")))
        .collect();
    fixtures.sort();

    // Group by what each one drew. One group means every feature decoded to the same picture.
    let mut groups: BTreeMap<(u32, u32, Vec<u8>), Vec<String>> = BTreeMap::new();
    let mut unrenderable = Vec::new();
    for path in &fixtures {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match render(path) {
            Some(image) => groups.entry(image).or_default().push(name),
            None => unrenderable.push(name),
        }
    }

    assert!(fixtures.len() >= 90, "expected the JBIG2 conformance fixtures in the pinned corpus, found {}", fixtures.len());
    assert!(unrenderable.is_empty(), "every fixture must render: {unrenderable:?}");

    if groups.len() > 1 {
        // Name the odd ones out: the largest group is what the picture should be, so anything
        // else is the bug, and saying which files they are is the whole value of the failure.
        let mut sized: Vec<(usize, &Vec<String>)> = groups.values().map(|names| (names.len(), names)).collect();
        sized.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        let report: Vec<String> = sized.iter().skip(1).map(|(n, names)| format!("{n} file(s): {names:?}")).collect();
        panic!(
            "JBIG2 coding features must not change the picture, but {} of {} fixtures decoded differently:\n  {}",
            fixtures.len().saturating_sub(sized.first().map_or(0, |(n, _)| *n)),
            fixtures.len(),
            report.join("\n  ")
        );
    }
}
