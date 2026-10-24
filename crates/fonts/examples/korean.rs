//! Feasibility check for the Korean craft face: sets Korean text in 학교안심 나무 (Hakgyoansim
//! Namu), writes a one-page PDF, and prints where it went. Run it against a craft-fonts checkout:
//!
//! ```sh
//! CRAFT_FONTS_DIR=../craft-fonts cargo run -p pdfcraft-fonts --example korean -- /tmp/namu-korean.pdf
//! ```

use std::error::Error;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use pdfcraft_fonts::pdf::Metrics;

/// `US Letter` in points.
const PAGE: (f64, f64) = (612.0, 792.0);

fn main() -> Result<(), Box<dyn Error>> {
    let Some(face) = pdfcraft_fonts::document_korean_font() else {
        return Err("built without a Korean craft face: build with CRAFT_FONTS_DIR=<craft-fonts checkout>".into());
    };
    let path = std::env::args().nth(1).unwrap_or_else(|| "namu-korean.pdf".to_owned());

    let mut doc = Document::new_empty();
    let mut fonts = Dict::new();
    let resource = pdfcraft_fonts::embedding::embed(&mut doc, &mut fonts, face.bytes, face.family).map_err(|e| e.to_string())?;
    let entry = fonts.get(resource.as_bytes()).ok_or("the embedding left no resource entry")?.clone();

    let lines = ["한국어 테스트 문서", "학교안심 나무 · Namu", "가나다라마바사아자차카타파하"];
    let body = {
        let font = doc.dict(&entry).ok_or("the embedded font dictionary is gone")?;
        let metrics = Metrics::from_dict(&doc, &font);
        let mut body = format!("BT\n/{resource} 24 Tf\n72 700 Td\n").into_bytes();
        for line in lines {
            let codes = metrics.encode(line).ok_or_else(|| format!("{} can't show: {line}", face.name()))?;
            body.push(b'<');
            for byte in &codes {
                body.extend(format!("{byte:02X}").into_bytes());
            }
            body.extend_from_slice(b"> Tj\n0 -40 Td\n");
        }
        body.extend_from_slice(b"ET\n");
        body
    };

    let content = doc.add(Object::Stream(Stream::flate(Dict::new(), &body)));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).ok_or("the new document has no page tree")?;
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), Object::Real(PAGE.0), Object::Real(PAGE.1)]));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Ref(content));
    let page_ref = doc.add(page);
    let _ = doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(page_ref));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    });

    let bytes = write_full(&doc, &SaveOptions::default())?;
    std::fs::write(&path, &bytes)?;
    println!("wrote {path} ({} bytes): {} set in {}", bytes.len(), lines.join(" / "), face.name());
    Ok(())
}
