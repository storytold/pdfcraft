//! Real full-source font regressions for logical ActualText selection geometry.
use super::*;
use crate::{PageRenderer, RenderConfig};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_fonts::source_shaping::ShapedRun;
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use std::sync::Arc;

struct SourceFixture {
    bytes: Vec<u8>,
    drawn: ShapedRun,
    advances: Vec<f64>,
}

fn approved_source_face() -> Option<Vec<u8>> {
    let font = pdfcraft_fonts::document_arabic_font()?;
    let bytes = default_static_test_face(font.bytes);
    pdfcraft_fonts::embedding::validate_document_input(&bytes).unwrap();
    Some(bytes)
}

fn default_static_test_face(bytes: &[u8]) -> Vec<u8> {
    let u16_at = |at: usize| u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let omitted = [*b"fvar", *b"gvar", *b"avar", *b"HVAR", *b"MVAR", *b"VVAR", *b"cvar", *b"STAT", *b"DSIG"];
    let mut tables = Vec::new();
    for index in 0..usize::from(u16_at(4)) {
        let at = 12 + index * 16;
        let tag: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        if omitted.contains(&tag) {
            continue;
        }
        let begin = u32_at(at + 8);
        let mut data = bytes[begin..begin + u32_at(at + 12)].to_vec();
        if tag == *b"head" {
            data[8..12].fill(0);
        }
        tables.push((tag, data));
    }
    tables.sort_unstable_by_key(|table| table.0);
    let count = u16::try_from(tables.len()).unwrap();
    let power = 1u16 << count.ilog2();
    let mut out = bytes[..4].to_vec();
    for value in [count, power * 16, count.ilog2() as u16, count * 16 - power * 16] {
        out.extend(value.to_be_bytes());
    }
    out.resize(12 + tables.len() * 16, 0);
    let checksum = |data: &[u8]| {
        data.chunks(4).fold(0u32, |sum, chunk| {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            sum.wrapping_add(u32::from_be_bytes(word))
        })
    };
    let mut head = None;
    for (index, (tag, data)) in tables.iter().enumerate() {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let offset = out.len();
        if tag == b"head" {
            head = Some(offset);
        }
        let mut entry = tag.to_vec();
        for value in [checksum(data), u32::try_from(offset).unwrap(), u32::try_from(data.len()).unwrap()] {
            entry.extend(value.to_be_bytes());
        }
        out[12 + index * 16..12 + (index + 1) * 16].copy_from_slice(&entry);
        out.extend(data);
    }
    let adjustment = 0xb1b0_afbau32.wrapping_sub(checksum(&out));
    let head = head.unwrap();
    out[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    out
}

fn empty_source_page() -> Document {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << >> >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

fn source_fixture(logical: &str, replacement: Option<&str>, fitted: bool) -> Option<SourceFixture> {
    source_fixture_with_text(logical, logical, replacement, fitted)
}

fn source_fixture_with_text(painted: &str, logical: &str, replacement: Option<&str>, fitted: bool) -> Option<SourceFixture> {
    let font = approved_source_face()?;
    let paragraph = pdfcraft_fonts::source_shaping::Paragraph::new(painted).unwrap();
    let mut drawn = if fitted {
        paragraph.shape_fitted(&font, 0..painted.len(), 0.0, 0.0, 0.0).unwrap()
    } else {
        paragraph.shape(&font, 0..painted.len()).unwrap()
    };
    // A valid source PDF can encode the font's real presentation-form lam-alef glyph as
    // logical "لا". Preserve that one physical glyph; do not fabricate two letter boxes.
    if painted != logical {
        for glyph in &mut drawn.glyphs {
            if glyph.text == "\u{fefb}" {
                glyph.text = "لا".into();
            }
        }
    }
    let face = FontRef::new(&font).unwrap();
    let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
    let em = f64::from(face.metrics(Size::unscaled(), LocationRef::default()).units_per_em);
    let advances = drawn.glyphs.iter().map(|glyph| f64::from(metrics.advance_width(glyph.id).unwrap_or(0.0)) / em).collect();
    let mut doc = empty_source_page();
    let mut fonts = Dict::new();
    let mappings: Vec<_> = drawn.glyphs.iter().map(|glyph| (glyph.text.clone(), glyph.id)).collect();
    let (family, _) = pdfcraft_fonts::embedding::names(&font).unwrap();
    let resource = pdfcraft_fonts::embedding::embed_mapping(&mut doc, &mut fonts, &font, &family, &mappings).unwrap();
    let top = fonts.get(resource.as_bytes()).and_then(|f| doc.dict(f)).unwrap();
    assert_eq!(top.name(b"Subtype"), Some(b"Type0".as_slice()), "test the complete source face, not an outline fallback");
    let actual: String = replacement.unwrap_or(logical).encode_utf16().map(|u| format!("{u:04X}")).collect();
    let mut content = format!("/Span <</ActualText <FEFF{actual}>>> BDC\n");
    for (index, glyph) in drawn.glyphs.iter().enumerate() {
        let x = 72.0 + glyph.position[0] * 22.0;
        let y = 650.0 + glyph.position[1] * 22.0;
        content.push_str(&format!("BT /{resource} 22 Tf 1 0 0 1 {x:.8} {y:.8} Tm <{:04X}> Tj ET\n", index + 1));
    }
    content.push_str("EMC\n");
    doc.set(ObjRef::new(4, 0), Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    doc.update_dict(ObjRef::new(3, 0), |page| {
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(saved)).unwrap();
    let bytes = write_full(&reopened, &SaveOptions::default()).unwrap();
    Some(SourceFixture { bytes, drawn, advances })
}

impl SourceFixture {
    fn extract(&self) -> PageText {
        PageRenderer::new(Arc::new(self.bytes.clone()), RenderConfig::default()).filtered_text(0, TextFilter::default()).unwrap()
    }

    fn painted_x(&self, glyph: usize) -> [f32; 2] {
        let source = self.drawn.glyphs.get(glyph).unwrap();
        let advance = self.advances.get(glyph).copied().unwrap();
        assert!(advance > 0.0, "the exact x-box oracle needs an advancing source glyph");
        let left = 72.0 + source.position[0] * 22.0;
        [left as f32, (left + advance * 22.0) as f32]
    }
}

fn assert_x(actual: [f32; 4], expected: [f32; 2]) {
    assert!((actual[0] - expected[0]).abs() < 0.02, "left source box {actual:?} != {expected:?}");
    assert!((actual[2] - expected[1]).abs() < 0.02, "right source box {actual:?} != {expected:?}");
}

#[test]
fn full_source_actual_text_words_and_digits_keep_real_boxes() {
    let logical = "سلام عربي 123 (PDF)";
    let Some(fixture) = source_fixture(logical, None, false) else { return };
    let text = fixture.extract();
    assert_eq!(text.plain_text(), logical);
    for word in ["سلام", "عربي", "123", "PDF"] {
        let found = text.find(word);
        assert_eq!(found.len(), 1);
        assert_eq!(text.text_of(found[0].clone()), word, "substring search must not capture its full ActualText line");
        let (begin, end) = text.word_at(found[0].start).unwrap();
        assert_eq!(text.text_of(begin..end + 1).trim_matches(['(', ')']), word, "double-click must select one word");
    }
    let digits = text.find("123").remove(0);
    for digit in ['1', '2', '3'] {
        let logical = text.glyphs.get(digits.clone()).unwrap().iter().find(|glyph| glyph.text == digit.to_string()).unwrap();
        let painted = fixture.drawn.glyphs.iter().position(|glyph| glyph.text == digit.to_string()).unwrap();
        assert_x(logical.rect, fixture.painted_x(painted));
    }
    assert!(
        text.glyphs.iter().map(|glyph| glyph.rect[2] - glyph.rect[0]).fold(f32::INFINITY, f32::min)
            < text.glyphs.iter().map(|glyph| glyph.rect[2] - glyph.rect[0]).fold(f32::NEG_INFINITY, f32::max) * 0.8,
        "full source selection boxes must retain actual nonuniform advances"
    );
}

#[test]
fn full_source_presentation_ligature_keeps_its_single_real_glyph_box() {
    let Some(fixture) = source_fixture_with_text("ABC \u{fefb} 123", "ABC لا 123", None, false) else { return };
    let ligature = fixture.drawn.glyphs.iter().position(|glyph| glyph.text == "لا").expect("approved source presentation-form lam-alef glyph");
    let text = fixture.extract();
    assert_eq!(text.plain_text(), "ABC لا 123");
    let found = text.find("لا").remove(0);
    assert_eq!(found.end - found.start, 1, "one painted ligature stays atomic");
    let selected = text.glyphs.get(found.start).unwrap();
    assert_eq!(selected.text, "لا");
    assert_x(selected.rect, fixture.painted_x(ligature));
}

#[test]
fn repeated_full_source_arabic_occurrences_keep_their_own_painted_positions() {
    let logical = "باب باب 123";
    let Some(fixture) = source_fixture(logical, None, false) else { return };
    let text = fixture.extract();
    assert_eq!(text.plain_text(), logical);
    let found = text.find("باب");
    assert_eq!(found.len(), 2);
    let boxes: Vec<_> = found.iter().map(|range| text.line_rects(range.clone())).collect();
    assert!(boxes.iter().all(|rects| rects.len() == 1));
    assert!(boxes[0][0][0] > boxes[1][0][2], "first logical Arabic occurrence is the right-hand painted word: {boxes:?}");
    let mut words: Vec<Vec<usize>> = vec![Vec::new()];
    for (index, glyph) in fixture.drawn.glyphs.iter().enumerate() {
        if glyph.text == " " {
            words.push(Vec::new());
        } else if glyph.text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff))
            || (glyph.text.is_empty() && words.last().is_some_and(|word| !word.is_empty()))
        {
            words.last_mut().unwrap().push(index);
        }
    }
    let mut painted: Vec<_> = words
        .into_iter()
        .filter(|word| word.iter().filter(|&&i| !fixture.drawn.glyphs[i].text.is_empty()).count() == 3)
        .map(|word| {
            // Dotted Arabic letters may attach Unicode to a zero-advance dot before their
            // empty-Unicode advancing base. The real source advance belongs to that base.
            let advancing: Vec<_> = word.into_iter().filter(|&i| fixture.advances[i] > 0.0).collect();
            let left = advancing.iter().map(|i| fixture.painted_x(*i)[0]).fold(f32::INFINITY, f32::min);
            let right = advancing.iter().map(|i| fixture.painted_x(*i)[1]).fold(f32::NEG_INFINITY, f32::max);
            [left, right]
        })
        .collect();
    painted.sort_by(|a, b| b[0].total_cmp(&a[0]));
    assert_eq!(painted.len(), 2);
    for (actual, expected) in boxes.iter().zip(painted) {
        assert_x(actual[0], expected);
    }
}

#[test]
fn vocalized_full_source_actual_text_keeps_marks_and_word_granularity() {
    let logical = "بِسْمِ 123 اللَّهِ (PDF)";
    let Some(fixture) = source_fixture(logical, None, false) else { return };
    assert!(fixture.drawn.glyphs.iter().any(|glyph| glyph.text.is_empty()), "exercise auxiliary mark glyphs without ToUnicode text");
    let text = fixture.extract();
    assert_eq!(text.plain_text(), logical);
    for word in ["بِسْمِ", "123", "اللَّهِ"] {
        let found = text.find(word);
        assert_eq!(found.len(), 1);
        assert_eq!(text.text_of(found[0].clone()), word);
        let (start, end) = text.word_at(found[0].start).unwrap();
        assert_eq!(text.text_of(start..end + 1), word);
        assert!(text.line_rects(found[0].clone()).iter().all(|rect| rect[2] - rect[0] < 150.0));
    }
}

#[test]
fn full_source_logical_bidi_selection_does_not_cover_unselected_painted_letters() {
    let logical = "ABC عربي DEF";
    let Some(fixture) = source_fixture(logical, None, false) else { return };
    let text = fixture.extract();
    assert_eq!(text.plain_text(), logical);
    let found = text.find("ABC ع").remove(0);
    assert_eq!(text.text_of(found.clone()), "ABC ع");
    let excluded = text.find("ر").remove(0);
    let glyph = text.glyphs.get(excluded.start).unwrap();
    let centre = [(glyph.rect[0] + glyph.rect[2]) * 0.5, (glyph.rect[1] + glyph.rect[3]) * 0.5];
    let rects = text.line_rects(found);
    assert!(rects.len() >= 2, "mixed bidi search needs disjoint paint rectangles: {rects:?}");
    assert!(
        !rects.iter().any(|rect| centre[0] > rect[0] && centre[0] < rect[2] && centre[1] > rect[1] && centre[1] < rect[3]),
        "search/redaction must not bridge over unselected Arabic letters: {rects:?}"
    );
}

#[test]
fn full_source_controls_and_hanging_spaces_retain_logical_text_without_a_blank_word() {
    let logical = "\u{2067}نص 123\u{2069}   ";
    let Some(fixture) = source_fixture(logical, None, true) else { return };
    let text = fixture.extract();
    assert_eq!(text.plain_text(), logical);
    let found = text.find("123").remove(0);
    assert_eq!(text.text_of(found.clone()), "123");
    let (begin, end) = text.word_at(found.start).unwrap();
    let word = text.text_of(begin..end + 1);
    assert_eq!(word.trim_matches(['\u{2067}', '\u{2069}']), "123");
    assert_eq!(text.line_of.iter().copied().collect::<std::collections::HashSet<_>>().len(), 1);
}

#[test]
fn unrelated_full_source_actual_text_remains_an_atomic_replacement() {
    let Some(fixture) = source_fixture("سَلَام 123", Some("replacement icon label"), false) else { return };
    let text = fixture.extract();
    assert_eq!(text.plain_text(), "replacement icon label");
    assert_eq!(text.glyphs.len(), 1, "unrelated replacement text has no honest per-character source geometry");
}

fn selection_glyph(text: &str, along: f32, degrees: f32) -> TextGlyph {
    let (s, c) = degrees.to_radians().sin_cos();
    let point = |x: f32, y: f32| [100.0 + c * x - s * y, 100.0 + s * x + c * y];
    let quad = [point(along, 2.0), point(along + 6.0, 2.0), point(along + 6.0, 0.0), point(along, 0.0)];
    let rect = quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
        [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
    });
    TextGlyph { text: text.into(), rect, quad, direction: [c, s] }
}

#[test]
fn disjoint_logical_selection_keeps_rotated_unselected_glyphs_outside_its_rectangles() {
    for degrees in [0.0, 30.0, 90.0, 180.0, 270.0, -45.0] {
        // Logical A,B are physically separated by the unselected RTL C,D run.
        let glyphs = [0.0, 30.0, 20.0, 10.0].into_iter().zip(["A", "B", "C", "D"]).map(|(at, text)| selection_glyph(text, at, degrees)).collect();
        let page = PageText { glyphs, line_of: vec![0; 4], space_before: vec![false; 4] };
        for (selected, excluded) in [(vec![0, 1], vec![2, 3]), (vec![0, 2], vec![1, 3])] {
            let rects = page.glyph_rects(&selected);
            assert_eq!(rects.len(), 2, "the unselected physical run separates rectangles at {degrees} degrees");
            assert_eq!(rects[0], page.glyphs[selected[0]].rect, "pieces retain their logical selection order");
            for index in excluded {
                let g = &page.glyphs[index];
                let (x, y) = ((g.rect[0] + g.rect[2]) * 0.5, (g.rect[1] + g.rect[3]) * 0.5);
                assert!(
                    !rects.iter().any(|r| x > r[0] && x < r[2] && y > r[1] && y < r[3]),
                    "selection covers excluded glyph {index} at {degrees} degrees"
                );
            }
        }
    }
}

#[test]
fn ordinary_english_skipped_glyph_still_separates_selection_rectangles() {
    let page = PageText {
        glyphs: ["A", "B", "C"].into_iter().enumerate().map(|(i, text)| selection_glyph(text, i as f32 * 10.0, 0.0)).collect(),
        line_of: vec![0; 3],
        space_before: vec![false; 3],
    };
    assert_eq!(page.glyph_rects(&[0, 2]), vec![page.glyphs[0].rect, page.glyphs[2].rect]);
    assert_eq!(page.line_rects(0..3).len(), 1, "a complete ordinary line remains continuous");
}

#[test]
fn over_budget_line_selection_keeps_disjoint_rectangles_conservatively() {
    let count = 5_000usize;
    let page = PageText {
        glyphs: (0..count).map(|i| selection_glyph("A", i as f32 * 10.0, 0.0)).collect(),
        line_of: vec![0; count],
        space_before: vec![false; count],
    };
    assert_eq!(page.glyph_rects(&[0, count - 1]), vec![page.glyphs[0].rect, page.glyphs[count - 1].rect]);
}
