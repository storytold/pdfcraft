//! Poster tile labels: each tile's page, row and column, printed in the margin above it.

use super::*;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

fn poster(pages: Vec<usize>, labels: bool) -> Settings {
    settings(pages, Layout::Poster { scale: 400.0, overlap: 36.0, cut_marks: true, labels })
}

#[test]
fn poster_tiles_are_labelled_with_their_place_in_the_poster() {
    let sheets = layout(&[(200.0, 300.0), (200.0, 300.0)], &poster(vec![1], true)).unwrap();
    let texts: Vec<&str> = sheets
        .iter()
        .map(|s| {
            assert_eq!(s.texts.len(), 1);
            s.texts[0].text.as_str()
        })
        .collect();
    assert_eq!(
        texts,
        [
            "Page 2: row 1 of 2, column 1 of 2",
            "Page 2: row 1 of 2, column 2 of 2",
            "Page 2: row 2 of 2, column 1 of 2",
            "Page 2: row 2 of 2, column 2 of 2"
        ],
        "row by row from the top-left, as the tiles are laid out"
    );
    for s in &sheets {
        let t = &s.texts[0];
        assert!(t.at.1 > s.size.1 - MARGIN && t.at.1 + t.size < s.size.1, "in the top margin: {t:?}");
        assert!(t.at.0 > MARGIN && t.at.0 + 0.6 * t.size * (t.text.len() as f64) < s.size.0 - MARGIN, "between the cut marks: {t:?}");
    }
    assert!(layout(&[(200.0, 300.0)], &poster(vec![0], false)).unwrap().iter().all(|s| s.texts.is_empty()), "labels are off unless asked for");
}

#[test]
fn a_label_shrinks_to_fit_a_narrow_sheet() {
    let narrow = Settings { paper: (150.0, 200.0), ..poster(vec![0], true) };
    let sheets = layout(&[(200.0, 300.0)], &narrow).unwrap();
    let t = &sheets[0].texts[0];
    assert!(t.size < 7.0 && t.size > 0.0, "{t:?}");
    assert!(t.at.0 + 0.6 * t.size * t.text.len() as f64 <= sheets[0].size.0 - MARGIN - 4.0 + 0.01, "{t:?}");
    let sliver =
        Settings { paper: (100.0, 200.0), ..settings(vec![0], Layout::Poster { scale: 100.0, overlap: 0.0, cut_marks: true, labels: true }) };
    assert!(layout(&[(200.0, 300.0)], &sliver).unwrap().iter().all(|s| s.texts.is_empty()), "too narrow for a readable label");
}

#[test]
fn printed_labels_read_back_above_each_tile() {
    read_back_labels(&poster(vec![0], true));
}

#[test]
fn printed_labels_on_a_narrow_sheet_stay_between_the_cut_marks() {
    let narrow = settings(vec![0], Layout::Poster { scale: 100.0, overlap: 0.0, cut_marks: true, labels: true });
    read_back_labels(&Settings { paper: (150.0, 200.0), ..narrow });
}

/// Renders each printed sheet of `settings` (a poster of the one-page fixture), finds the label
/// its layout gives it and checks every glyph sits in the top margin, between the corners' cut
/// marks.
fn read_back_labels(settings: &Settings) {
    let sheets = layout(&[(200.0, 300.0)], settings).unwrap();
    assert!(sheets.len() > 1 && sheets.iter().all(|s| s.texts.len() == 1), "one label a tile");
    let printed = impose(&fixture(1), settings).unwrap();
    let mut renderer = PageRenderer::new(Arc::new(printed), RenderConfig::default());
    for (page, sheet) in sheets.iter().enumerate() {
        let r = renderer.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        assert_eq!(r.error, None);
        let text = r.text.unwrap();
        let found = text.find(&sheet.texts[0].text);
        assert_eq!(found.len(), 1, "sheet {page}: {}", text.plain_text());
        let rects = text.glyph_rects(&found[0].clone().collect::<Vec<_>>());
        let right = (sheet.size.0 - MARGIN) as f32;
        for r in rects {
            // View space is y down: the top margin is y in 0..MARGIN.
            assert!(r[1] >= 0.0 && r[3] <= MARGIN as f32 && r[0] > MARGIN as f32 && r[2] < right, "sheet {page}: glyph at {r:?}");
        }
    }
}

#[test]
fn label_text_is_escaped_for_a_literal_string() {
    assert_eq!(crate::imposition::literal(r"a (b) \c é"), r"a \(b\) \\c ?");
}
