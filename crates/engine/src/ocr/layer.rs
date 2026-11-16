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

/// A right-to-left letter (Hebrew, Arabic and the other right-to-left blocks), by the same
/// rule the text extractor uses. The Arabic-Indic digits lie in those blocks but are numbers.
fn is_rtl_letter(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF) && !is_arabic_digit(c)
}

fn is_arabic_digit(c: char) -> bool {
    matches!(c, '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}')
}

/// A character that keeps its left-to-right order inside right-to-left text: a digit or a
/// letter of a left-to-right script.
fn keeps_order(c: char) -> bool {
    is_arabic_digit(c) || (c.is_alphanumeric() && !is_rtl_letter(c))
}

/// The word's characters in the order they are drawn, left to right. A recognized word holds
/// its text in logical order (the order it is read and typed in), but a PDF shows text in
/// visual order, and that is what every extractor expects, this one included: for a
/// right-to-left word the first letter is the rightmost. So a word with a right-to-left
/// letter is drawn back to front, except that a number or a left-to-right word inside it
/// (with a single separator between two of its characters, as in 12.5 or 2026/11) keeps its
/// own order. Without this an Arabic or Hebrew word read back, searched or copied comes out
/// reversed. A word with no right-to-left letter is drawn as it is.
fn visual_order(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    if !chars.iter().copied().any(is_rtl_letter) {
        return chars;
    }
    // Runs in logical order: a left-to-right run stays whole, anything else is one character.
    let mut runs: Vec<Vec<char>> = Vec::new();
    let mut in_run = false;
    let mut rest = chars.iter().copied().peekable();
    while let Some(c) = rest.next() {
        let separator = in_run && matches!(c, '.' | ',' | ':' | '/' | '-' | '\u{066B}' | '\u{066C}') && rest.peek().copied().is_some_and(keeps_order);
        if in_run && (keeps_order(c) || separator) {
            if let Some(run) = runs.last_mut() {
                run.push(c);
            }
        } else {
            in_run = keeps_order(c);
            runs.push(vec![c]);
        }
    }
    runs.into_iter().rev().flatten().collect()
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
    let chars: Vec<char> = words.iter().flat_map(|w| w.text.chars()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
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
    for w in words {
        let glyphs = visual_order(&w.text);
        let count = glyphs.len();
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
        for (i, ch) in glyphs.iter().enumerate() {
            let Some(&(font, code)) = codes.get(ch) else { continue };
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
    }
    content.push_str("ET\nQ\nEMC\n");
    pdfcraft_edit::stamp(doc, page, "OCR", content.into_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::visual_order;

    fn drawn(text: &str) -> String {
        visual_order(text).into_iter().collect()
    }

    /// Right-to-left words are drawn back to front; numbers and left-to-right words inside
    /// them keep their order; everything else is untouched.
    #[test]
    fn right_to_left_words_are_drawn_in_visual_order() {
        assert_eq!(drawn("بسم"), "مسب");
        assert_eq!(drawn("שלום"), "םולש");
        assert_eq!(drawn("برفضه."), ".هضفرب", "the full stop ends the word: it is drawn at its left");
        // Unchanged: no right-to-left letter.
        for same in ["", "Contract", "2026", "12.5%", "日本語", "٢٠٢٦", "naïve"] {
            assert_eq!(drawn(same), same);
        }
        // A number inside right-to-left text reads left to right, separators included.
        assert_eq!(drawn("م2026"), "2026م");
        assert_eq!(drawn("2026م"), "م2026");
        assert_eq!(drawn("رقم12.5"), "12.5مقر");
        assert_eq!(drawn("١٢٣٤هـ"), "ـه١٢٣٤");
        // So does a left-to-right word, and a separator that ends one is not part of it.
        assert_eq!(drawn("ملفPDF"), "PDFفلم");
        assert_eq!(drawn("ال-PDF"), "PDF-لا");
        assert_eq!(drawn("12-ب"), "ب-12");
        // A mark stays with its letter once the extractor turns the word back round.
        let marked = drawn("بَ");
        assert_eq!(marked.chars().rev().collect::<String>(), "بَ");
        // Drawing twice is not the identity in general, but the length never changes.
        for text in ["بسم الله", "a", "ب", "12ب34", "ב1.2.3ג"] {
            assert_eq!(visual_order(text).len(), text.chars().count(), "{text}");
        }
    }
}
