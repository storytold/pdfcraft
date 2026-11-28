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

/// How a character takes part in the order a word is drawn in. The classes follow the ones the
/// text extractor puts a line back into reading order by (`reverse_rtl` in `pdfcraft-render`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A letter, mark or punctuation of a right-to-left script (Hebrew, Arabic and the others).
    Rtl,
    /// A numeric character (the Arabic-Indic digits included), and what belongs to its number.
    Number,
    /// A letter of a left-to-right script, and what reads on with it.
    Letter,
    /// Everything else: punctuation, symbols, marks.
    Other,
}

fn kind(c: char) -> Kind {
    let rtl_block = matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF);
    if c.is_numeric() {
        Kind::Number
    } else if rtl_block {
        Kind::Rtl
    } else if c.is_alphabetic() {
        Kind::Letter
    } else {
        Kind::Other
    }
}

/// The word's characters in the order they are drawn, left to right. A recognized word holds
/// its text in logical order (the order it is read and typed in), but a PDF shows text in
/// visual order, and that is what the text extractor expects: for a right-to-left word the
/// first letter is the rightmost. So a word with a right-to-left character (a letter, or
/// punctuation of a right-to-left script such as the Arabic comma) is drawn back to front,
/// except that a number or a left-to-right word inside it keeps its own order, by the rules
/// the extractor reads them back with in a right-to-left line. Without this an Arabic or
/// Hebrew word read back, searched or copied comes out reversed. A word with no
/// right-to-left character is drawn as it is.
///
/// A word is ordered on its own, as part of a right-to-left line; the caller leaves a page
/// with no right-to-left letter alone. Where the extractor goes by the rest of the line (a
/// left-to-right line on a page that also has right-to-left text, or a left-to-right word
/// next to this one), a right-to-left word's letters still read back in order, but what is
/// attached to them (punctuation, a number) can come back on its other side.
fn visual_order(text: &str) -> Vec<char> {
    let mut chars: Vec<char> = text.chars().collect();
    let mut kinds: Vec<Kind> = chars.iter().copied().map(kind).collect();
    if !kinds.contains(&Kind::Rtl) {
        return chars;
    }
    let n = chars.len();
    // A single separator between two digits is part of the number: 12.5, 16/11/2026.
    for i in 1..n.saturating_sub(1) {
        let separator = matches!(chars[i], '.' | ',' | ':' | '/' | '+' | '-' | '\u{066B}' | '\u{066C}');
        if separator && kinds[i - 1] == Kind::Number && kinds[i + 1] == Kind::Number {
            kinds[i] = Kind::Number;
        }
    }
    // So is a percent or currency sign next to it, on either side: 15%, $20. (Not the Arabic
    // percent sign: it is a right-to-left character and reads on with the letters.)
    let sign = |c: char| matches!(c, '%' | '‰' | '#' | '$' | '¢'..='¥' | '°' | '\u{20A0}'..='\u{20CF}');
    for i in 1..n {
        if kinds[i] == Kind::Other && sign(chars[i]) && kinds[i - 1] == Kind::Number {
            kinds[i] = Kind::Number;
        }
    }
    for i in (0..n.saturating_sub(1)).rev() {
        if kinds[i] == Kind::Other && sign(chars[i]) && kinds[i + 1] == Kind::Number {
            kinds[i] = Kind::Number;
        }
    }
    // A number after a left-to-right letter reads on with it (A4, v2)...
    let mut after_letter = false;
    for kind in &mut kinds {
        match *kind {
            Kind::Letter => after_letter = true,
            Kind::Rtl => after_letter = false,
            Kind::Number if after_letter => *kind = Kind::Letter,
            Kind::Number | Kind::Other => {}
        }
    }
    // ...and so does what stands between two of them (a hyphen, a bracket).
    let mut from = 0;
    while let Some(first) = (from..n).find(|&i| kinds[i] == Kind::Other) {
        let end = (first..n).find(|&i| kinds[i] != Kind::Other).unwrap_or(n);
        if first > 0 && end < n && kinds[first - 1] == Kind::Letter && kinds[end] == Kind::Letter {
            kinds[first..end].fill(Kind::Letter);
        }
        from = end;
    }
    // Back to front, each left-to-right stretch then put back in its own order.
    chars.reverse();
    kinds.reverse();
    let keeps_order = |kind: Kind| matches!(kind, Kind::Number | Kind::Letter);
    let mut from = 0;
    while let Some(first) = (from..n).find(|&i| keeps_order(kinds[i])) {
        let end = (first..n).find(|&i| !keeps_order(kinds[i])).unwrap_or(n);
        chars[first..end].reverse();
        from = end;
    }
    chars
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
    // Only a page that has a right-to-left letter is reordered. On any other page the extractor
    // reads every line left to right, so each word is drawn as it was recognized, a stray
    // right-to-left punctuation mark included.
    let right_to_left = words.iter().any(|w| w.text.chars().any(|c| kind(c) == Kind::Rtl && c.is_alphabetic()));
    for w in words {
        let glyphs: Vec<char> = if right_to_left { visual_order(&w.text) } else { w.text.chars().collect() };
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
