//! Lossless scan correction: straighten a page's content in place (Scan & OCR ▸ Correct
//! scanned pages). The OCR text layer itself is written by `ocr::layer`.
use crate::EditError;
use pdfcraft_cos::{Dict, Document, Object, Stream};

/// Whether `page` has annotations: deskewing would leave them where the content was.
pub(crate) fn has_annotations(doc: &Document, page: &pdfcraft_model::Page) -> bool {
    page.dict.get(b"Annots").map(|v| doc.resolve(v)).and_then(|v| v.as_array().map(|a| !a.is_empty())).unwrap_or(false)
}

/// Turn page `page`'s content by `degrees` (-12..12) about its crop box centre, scaled to stay
/// inside it. Pages with annotations are refused: their rectangles would not follow.
pub(crate) fn deskew(doc: &mut Document, page: usize, degrees: f64) -> Result<(), EditError> {
    if !degrees.is_finite() || degrees.abs() > 12.0 {
        return Err(EditError::Invalid("deskew must be between -12 and 12 degrees".into()));
    }
    let p = pdfcraft_model::pages(doc).get(page).cloned().ok_or_else(|| EditError::Invalid("no such page".into()))?;
    if has_annotations(doc, &p) {
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
        let word = pdfcraft_ocr::PlacedWord {
            text: "Été 中文".into(),
            origin: [10.0, 20.0],
            across: [90.0, 0.0],
            up: [0.0, 12.0],
            confidence: None,
            source: "ocrs".into(),
        };
        crate::ocr::layer::add_text(&mut doc, 0, &[word]).unwrap();
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
