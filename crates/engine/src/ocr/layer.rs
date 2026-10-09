//! Unicode search text has no visible glyphs. A generated Type 3 font supplies advance widths
//! and a ToUnicode map, with empty character programs; no display font is needed or embedded.

use std::collections::BTreeMap;

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_edit::EditError;
use pdfcraft_ocr::PlacedWord;

pub(crate) fn add_text(doc: &mut Document, page: usize, words: &[PlacedWord]) -> Result<(), EditError> {
    let length = words.iter().try_fold(0usize, |n, w| n.checked_add(w.text.len()));
    if length.is_none_or(|n| n > 4 * 1024 * 1024) || words.len() > 65536 {
        return Err(EditError::Invalid("OCR text layer is too large".into()));
    }
    if words.iter().any(|w| !w.origin.iter().chain(&w.across).chain(&w.up).all(|v| v.is_finite())) {
        return Err(EditError::Invalid("OCR text coordinates must be finite".into()));
    }
    // Keep the existing Latin layout, including Helvetica's proportional advance widths.
    if words.iter().all(|w| {
        w.text
            .chars()
            .all(|c| matches!(c, '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' | '€' | '‚' | '„' | '…' | '‘' | '’' | '“' | '”' | '•' | '–' | '—' | '™'))
    }) {
        return pdfcraft_edit::stamp(doc, page, "OCR", pdfcraft_ocr::text_layer(words));
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
    let mut content = String::from("/OCR BMC\nBT\n3 Tr\n");
    for w in words {
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
    }
    content.push_str("ET\nEMC\n");
    pdfcraft_edit::stamp(doc, page, "OCR", content.into_bytes())
}
