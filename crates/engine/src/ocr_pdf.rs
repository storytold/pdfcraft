//! Lossless scan correction and Unicode-only invisible OCR fonts (no font assets).
use crate::EditError;
use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_ocr::PlacedWord;
use std::collections::{BTreeMap, BTreeSet};

fn numbers(values: impl IntoIterator<Item = f64>) -> Object {
    Object::Array(values.into_iter().map(Object::Real).collect())
}

pub(crate) fn add_text(doc: &mut Document, page: usize, words: &[PlacedWord]) -> Result<(), EditError> {
    let p = pdfcraft_model::pages(doc).get(page).cloned().ok_or_else(|| EditError::Invalid("no such page".into()))?;
    if words.iter().any(|w| w.text.len() > 100_000 || w.origin.iter().chain(&w.across).chain(&w.up).any(|v| !v.is_finite())) {
        return Err(EditError::Invalid("invalid OCR word geometry or text".into()));
    }
    let chars: Vec<char> = words.iter().flat_map(|w| w.text.chars()).collect::<BTreeSet<_>>().into_iter().collect();
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts = res.get(b"Font").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut codes = BTreeMap::new();
    // Empty Type 3 glyphs with explicit metrics and ToUnicode keep text selectable without
    // shipping any typeface. Chunks also handle pages with more than 255 unique characters.
    for (index, chunk) in chars.chunks(255).enumerate() {
        let mut name = format!("PCOcr{index}");
        while fonts.contains(name.as_bytes()) {
            name.push('_');
        }
        let glyph = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1000 0 0 -210 1000 720 d1\n".to_vec())));
        let mut procs = Dict::new();
        let mut differences = vec![Object::Int(1)];
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (PdfCraft) /Ordering (Unicode) /Supplement 0 >> def\n/CMapName /PdfCraftOCR def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
        );
        for entries in chunk.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", entries.len()));
            for ch in entries {
                let code = codes.len() % 255 + 1;
                let g = format!("g{code}");
                procs.set(g.as_bytes().to_vec(), Object::Ref(glyph));
                differences.push(Object::name(&g));
                let mut utf16 = [0u16; 2];
                let hex: String = ch.encode_utf16(&mut utf16).iter().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{code:02X}> <{hex}>\n"));
                codes.insert(*ch, (name.clone(), code));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        let cmap = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
        let mut encoding = Dict::new();
        encoding.set(b"Type".to_vec(), Object::name("Encoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(differences));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type3"));
        font.set(b"FontBBox".to_vec(), numbers([0.0, -210.0, 1000.0, 720.0]));
        font.set(b"FontMatrix".to_vec(), numbers([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]));
        font.set(b"FirstChar".to_vec(), Object::Int(1));
        font.set(b"LastChar".to_vec(), Object::Int(chunk.len() as i64));
        font.set(b"Widths".to_vec(), numbers(std::iter::repeat_n(1000.0, chunk.len())));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        font.set(b"CharProcs".to_vec(), Object::Dict(procs));
        font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
        font.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        fonts.set(name.into_bytes(), Object::Ref(doc.add(Object::Dict(font))));
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).map_err(|e| EditError::Invalid(e.to_string()))?;
    // ET does not reset text rendering mode or the font. Keep our invisible Unicode
    // layer isolated, including when another producer later appends page content.
    let mut out = String::from("/OCR BMC\nq\nBT\n3 Tr\n");
    for word in words {
        let len = word.text.chars().count() as f64;
        if len == 0.0 {
            continue;
        }
        let [a, b] = word.across.map(|v| v / len);
        let [c, d] = word.up.map(|v| v / 0.93);
        let (x, y) = (word.origin[0] + c * 0.21, word.origin[1] + d * 0.21);
        out.push_str(&format!("{a:.6} {b:.6} {c:.6} {d:.6} {x:.6} {y:.6} Tm\n"));
        for ch in word.text.chars() {
            if let Some((name, code)) = codes.get(&ch) {
                out.push_str(&format!("/{name} 1 Tf <{code:02X}> Tj\n"));
            }
        }
    }
    out.push_str("ET\nQ\nEMC\n");
    pdfcraft_edit::stamp(doc, page, "OCR", out.into_bytes())?;
    Ok(())
}

pub(crate) fn deskew(doc: &mut Document, page: usize, degrees: f64) -> Result<(), EditError> {
    if !degrees.is_finite() || degrees.abs() > 12.0 {
        return Err(EditError::Invalid("deskew must be between -12 and 12 degrees".into()));
    }
    let p = pdfcraft_model::pages(doc).get(page).cloned().ok_or_else(|| EditError::Invalid("no such page".into()))?;
    if p.dict.get(b"Annots").map(|v| doc.resolve(v)).and_then(|v| v.as_array().map(|a| !a.is_empty())).unwrap_or(false) {
        return Err(EditError::Invalid("deskew is not available on pages with annotations; disable deskew or flatten annotations first".into()));
    }
    let [l, b, r, t] = p.crop(doc);
    let (w, h) = (r - l, t - b);
    if w <= 0.0 || h <= 0.0 {
        return Err(EditError::Invalid("invalid page bounds".into()));
    }
    let correction = pdfcraft_ocr::scan::Correction { rotation: 0, deskew: degrees };
    let k = correction.scale(w, h);
    let (s, c) = (-degrees).to_radians().sin_cos();
    let (a, bm, cm, d) = (k * c, k * s, -k * s, k * c);
    let (cx, cy) = ((l + r) / 2.0, (b + t) / 2.0);
    let (e, f) = (cx - a * cx - cm * cy, cy - bm * cx - d * cy);
    let open = format!("q\n{a:.9} {bm:.9} {cm:.9} {d:.9} {e:.9} {f:.9} cm\n{l} {b} {w} {h} re W n\n");
    let open = Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), open.as_bytes()))));
    let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"Q\n".to_vec()))));
    let original = p.dict.get(b"Contents").cloned();
    let mut streams = vec![open];
    if let Some(content) = original {
        if let Some(list) = doc.resolve(&content).as_array() {
            streams.extend(list.iter().cloned());
        } else {
            streams.push(content);
        }
    }
    streams.push(close);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(streams))).map_err(|e| EditError::Invalid(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_ocr_layer_restores_graphics_state() {
        let session = crate::Session::new();
        let bytes = session.create_from_text("OCR state", "Original text").unwrap();
        let mut doc = Document::open(bytes).unwrap();
        add_text(&mut doc, 0, &[PlacedWord { text: "Été 中文".into(), origin: [10.0, 20.0], across: [90.0, 0.0], up: [0.0, 12.0] }]).unwrap();
        let page = pdfcraft_model::pages(&doc).remove(0);
        let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
        let last = doc.resolve(contents.as_array().unwrap().last().unwrap());
        let Object::Stream(stream) = &*last else { panic!("OCR content is a stream") };
        let bytes = stream.decoded().unwrap();
        let content = std::str::from_utf8(&bytes).unwrap();
        let mut depth = 0_i32;
        for token in content.split_ascii_whitespace() {
            match token {
                "q" => depth += 1,
                "Q" => depth -= 1,
                "Tr" | "Tf" | "Tm" => assert!(depth > 0, "OCR {token} leaks into later page content"),
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
    }
}
