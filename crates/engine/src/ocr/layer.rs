//! Unicode search text has no visible glyphs. A generated Type 3 font supplies advance widths
//! and a ToUnicode map, with empty character programs; no display font is needed or embedded.
//! Adapted from the searchable-layer implementation in #433 (32e09fbe).

use std::collections::BTreeMap;

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_edit::EditError;
use pdfcraft_ocr::PlacedWord;

/// Refuse oversized text or invalid geometry before cloning words or allocating fonts.
/// These limits apply to the complete input, including all editable export pages.
pub(crate) fn validate_words<'a>(words: impl IntoIterator<Item = &'a PlacedWord>) -> Result<usize, EditError> {
    let mut count = 0usize;
    let mut length = 0usize;
    for w in words {
        count += 1;
        length = length.checked_add(w.text.len()).ok_or_else(|| EditError::Invalid("OCR text layer is too large".into()))?;
        if count > 65536 || length > 4 * 1024 * 1024 {
            return Err(EditError::Invalid("OCR text layer is too large".into()));
        }
        if !w.origin.iter().chain(&w.across).chain(&w.up).all(|v| v.is_finite())
            || !w.across[0].hypot(w.across[1]).is_finite()
            || !w.up[0].hypot(w.up[1]).is_finite()
            || (0..2).any(|i| !(w.origin[i] + 0.2 * w.up[i]).is_finite() || !(w.origin[i] + w.across[i] + 0.2 * w.up[i]).is_finite())
        {
            return Err(EditError::Invalid("OCR text coordinates must be finite".into()));
        }
    }
    Ok(count)
}

pub(crate) fn add_text(doc: &mut Document, page: usize, words: &[PlacedWord]) -> Result<(), EditError> {
    validate_words(words)?;
    // Keep the existing Latin layout, including Helvetica's proportional advance widths.
    if words.iter().all(|w| {
        w.text
            .chars()
            .all(|c| matches!(c, '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' | '€' | '‚' | '„' | '…' | '‘' | '’' | '“' | '”' | '•' | '–' | '—' | '™'))
    }) {
        pdfcraft_edit::stamp(doc, page, "OCR", pdfcraft_ocr::text_layer(words, &pdfcraft_fonts::TextPlan::default()))?;
        return Ok(());
    }
    let p = pdfcraft_model::pages(doc).get(page).cloned().ok_or_else(|| EditError::Invalid("OCR page does not exist".into()))?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let chars: Vec<char> =
        words.iter().flat_map(|w| w.text.chars()).chain(std::iter::once(' ')).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    // Match the extractor's em box (-200..800). Baselines below are offset by 0.2 em,
    // so search/selection rectangles span the detected image box.
    let blank = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1000 0 0 -200 1000 800 d1\n".to_vec())));
    let mut codes = BTreeMap::new();
    let mut names = Vec::new();
    for (index, chunk) in chars.chunks(255).enumerate() {
        let mut name = format!("PCOcr{index}");
        while fonts.contains(name.as_bytes()) {
            name.push('x');
        }
        let mut differences = vec![Object::Int(1)];
        let mut procs = Dict::new();
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (PdfCraft) /Ordering (Unicode) /Supplement 0 >> def\n/CMapName /PdfCraftOCR def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
        );
        for (batch, part) in chunk.chunks(100).enumerate() {
            cmap.push_str(&format!("{} beginbfchar\n", part.len()));
            for (i, ch) in part.iter().enumerate() {
                let code = (batch * 100 + i + 1) as u8;
                let glyph = format!("g{code:02X}");
                differences.push(Object::name(&glyph));
                procs.set(glyph.into_bytes(), Object::Ref(blank));
                let mut units = [0u16; 2];
                let hex: String = ch.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{code:02X}> <{hex}>\n"));
                codes.insert(*ch, (index, code));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        let map = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
        let mut encoding = Dict::new();
        encoding.set(b"Type".to_vec(), Object::name("Encoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(differences));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type3"));
        font.set(b"FontBBox".to_vec(), Object::Array(vec![0.into(), (-200).into(), 1000.into(), 800.into()]));
        font.set(b"FontMatrix".to_vec(), Object::Array(vec![Object::Real(0.001), 0.into(), 0.into(), Object::Real(0.001), 0.into(), 0.into()]));
        font.set(b"FirstChar".to_vec(), Object::Int(1));
        font.set(b"LastChar".to_vec(), Object::Int(chunk.len() as i64));
        font.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(1000); chunk.len()]));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        font.set(b"CharProcs".to_vec(), Object::Dict(procs));
        font.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        font.set(b"ToUnicode".to_vec(), Object::Ref(map));
        fonts.set(name.as_bytes().to_vec(), Object::Ref(doc.add(Object::Dict(font))));
        names.push(name);
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: like
    // `pdfcraft_ocr::text_layer` (#3029), the text is drawn inside `q … Q` so the invisible
    // rendering mode cannot leak into later content streams on the page.
    let mut content = String::from("/OCR BMC\nq\nBT\n3 Tr\n");
    for (word_index, w) in words.iter().enumerate() {
        let count = w.text.chars().count();
        if count == 0 || w.up[0].hypot(w.up[1]) <= 0.0 || w.across[0].hypot(w.across[1]) <= 0.0 {
            continue;
        }
        let mut start = 0;
        let mut run = Vec::new();
        let mut font_index = None;
        let emit = |content: &mut String, font: usize, start: usize, run: &[u8]| {
            let n = count as f64;
            let at = start as f64 / n;
            let hex: String = run.iter().map(|b| format!("{b:02X}")).collect();
            content.push_str(&format!(
                "/{} 1 Tf\n{} {} {} {} {} {} Tm <{hex}> Tj\n",
                names[font],
                w.across[0] / n,
                w.across[1] / n,
                w.up[0],
                w.up[1],
                w.origin[0] + at * w.across[0] + 0.2 * w.up[0],
                w.origin[1] + at * w.across[1] + 0.2 * w.up[1]
            ));
        };
        for (i, ch) in w.text.chars().enumerate() {
            let Some(&(font, code)) = codes.get(&ch) else { continue };
            if let Some(previous) = font_index
                && previous != font
            {
                emit(&mut content, previous, start, &run);
                run.clear();
                start = i;
            }
            font_index = Some(font);
            run.push(code);
        }
        if let Some(font) = font_index {
            emit(&mut content, font, start, &run);
        }
        if let Some(next) = words.get(word_index.saturating_add(1))
            && let Some(space) = pdfcraft_ocr::word_separator(w, next)
            && let Some(&(font, code)) = codes.get(&' ')
        {
            content.push_str(&format!(
                "/{} 1 Tf\n{} {} {} {} {} {} Tm <{code:02X}> Tj\n",
                names[font],
                space.across[0],
                space.across[1],
                space.up[0],
                space.up[1],
                space.origin[0] + 0.2 * space.up[0],
                space.origin[1] + 0.2 * space.up[1]
            ));
        }
    }
    content.push_str("ET\nQ\nEMC\n");
    pdfcraft_edit::stamp(doc, page, "OCR", content.into_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Recognizers report separate words even when their image boxes nearly touch. The
    /// searchable layer must retain that boundary without resizing any letter's box.
    #[test]
    fn unicode_ocr_keeps_explicit_word_boundaries_and_character_boxes() {
        let mut doc = Document::open(Arc::new(crate::tests::fixture(1))).unwrap();
        let word = |text: &str, x: f64, y: f64, width: f64| PlacedWord {
            text: text.into(),
            origin: [x, y],
            across: [width, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "synthetic".into(),
        };
        let words = [word("çÇ", 20.0, 250.0, 12.0), word("GLİÖŞ", 32.1, 250.0, 30.0), word("Çağrı", 20.0, 230.0, 30.0)];
        add_text(&mut doc, 0, &words).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        assert!(text.plain_text().contains("çÇ GLİÖŞ\nÇağrı"), "{}", text.plain_text());
        assert_eq!(text.find("çÇ GLİÖŞ").len(), 1);
        let letters: Vec<_> = text.glyphs.iter().filter(|g| g.rect[1] < 80.0 && !g.text.trim().is_empty()).collect();
        assert_eq!(letters.len(), 12, "letters remain individual selectable glyphs");
        let g = letters.iter().find(|g| g.text == "G").unwrap();
        assert!((g.rect[0] - 32.1).abs() < 0.001 && (g.rect[2] - 38.1).abs() < 0.001, "the letter's original box is unchanged: {g:?}");
    }

    #[test]
    fn win_ansi_ocr_keeps_explicit_word_boundaries() {
        let mut doc = Document::open(Arc::new(crate::tests::fixture(1))).unwrap();
        let words: Vec<_> = [("ab", 20.0), ("CD", 32.1)]
            .into_iter()
            .map(|(text, x)| PlacedWord {
                text: text.into(),
                origin: [x, 250.0],
                across: [12.0, 0.0],
                up: [0.0, 10.0],
                confidence: None,
                source: "synthetic".into(),
            })
            .collect();
        add_text(&mut doc, 0, &words).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        assert!(text.plain_text().contains("ab CD"), "{}", text.plain_text());
    }

    #[test]
    fn editable_ocr_keeps_separate_recognized_word_boundaries() {
        let mut session = crate::Session::new();
        let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
        let words = [("ab", 20.0), ("CD", 32.1)]
            .into_iter()
            .map(|(text, x)| PlacedWord {
                text: text.into(),
                origin: [x, 250.0],
                across: [12.0, 0.0],
                up: [0.0, 10.0],
                confidence: None,
                source: "synthetic".into(),
            })
            .collect();
        let found = [crate::ocr::OcrPage { already_on_page: 0, page: 0, words, skipped: None, notes: Vec::new() }];
        let bytes = session.editable_ocr_bytes(id, &found).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, pdfcraft_render::RenderConfig::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        assert!(text.plain_text().contains("ab CD"), "{}", text.plain_text());
    }

    #[test]
    fn rotated_unicode_ocr_keeps_word_boundaries() {
        for u in [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]] {
            let mut doc = Document::open(Arc::new(crate::tests::fixture(1))).unwrap();
            let words: Vec<_> = [("çÇ", 0.0, 12.0), ("GLİÖŞ", 12.1, 30.0)]
                .into_iter()
                .map(|(text, offset, width)| PlacedWord {
                    text: text.into(),
                    origin: [100.0 + offset * u[0], 220.0 + offset * u[1]],
                    across: [width * u[0], width * u[1]],
                    up: [-10.0 * u[1], 10.0 * u[0]],
                    confidence: None,
                    source: "synthetic".into(),
                })
                .collect();
            add_text(&mut doc, 0, &words).unwrap();
            let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
            let text = renderer.filtered_text(0, Default::default()).unwrap();
            assert!(text.plain_text().contains("çÇ GLİÖŞ"), "direction {u:?}: {}", text.plain_text());
        }
    }

    #[test]
    fn cjk_ocr_boundaries_do_not_invent_spaces() {
        let mut doc = Document::open(Arc::new(crate::tests::fixture(1))).unwrap();
        let words: Vec<_> = [("日", 20.0), ("本", 32.1)]
            .into_iter()
            .map(|(text, x)| PlacedWord {
                text: text.into(),
                origin: [x, 250.0],
                across: [12.0, 0.0],
                up: [0.0, 10.0],
                confidence: None,
                source: "synthetic".into(),
            })
            .collect();
        add_text(&mut doc, 0, &words).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        assert!(text.plain_text().contains("日本"), "{}", text.plain_text());
        assert!(!text.plain_text().contains("日 本"), "{}", text.plain_text());
    }
}
