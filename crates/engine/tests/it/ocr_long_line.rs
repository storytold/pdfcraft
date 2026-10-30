//! A line wider than the PP-OCR recogniser used to fail the whole page. The recogniser refuses a
//! crop whose width, scaled to its 48 px input, exceeds 4096 px (about 85 heights). A newspaper
//! line that the detector groups across columns is that shape. These tests keep the page.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use pdfcraft_engine::Session;
use pdfcraft_engine::export;
use pdfcraft_engine::ocr::{self, OcrSettings};

/// One tight line of words. The image is only a little taller than the glyphs, so even a line
/// box around the whole image is past the recogniser's limit.
fn wide_line_png(text: &str) -> Vec<u8> {
    let font = FontRef::try_from_slice(include_bytes!("../../../../assets/fonts/Inter-Regular.ttf")).unwrap();
    let size = 36.0;
    let scaled = font.as_scaled(PxScale::from(size));
    let mut advance = 24.0_f32;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        assert_ne!(id.0, 0, "fixture font must cover {ch}");
        advance += scaled.h_advance(id);
    }
    advance += 24.0;
    let width = advance.ceil() as u32;
    let height = 52u32;
    assert!(width as f32 / height as f32 > 100.0, "the fixture is {width}x{height}; a box around it would not have been refused");
    let mut rgba = vec![255u8; width as usize * height as usize * 4];
    let mut x = 24.0_f32;
    let baseline = 40.0_f32;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        let glyph = id.with_scale_and_position(size, point(x, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 {
                    return;
                }
                let (px, py) = (px as u32, py as u32);
                if px >= width || py >= height {
                    return;
                }
                let v = (255.0 * (1.0 - coverage)) as u8;
                let i = (py as usize * width as usize + px as usize) * 4;
                if let Some(pixel) = rgba.get_mut(i..i + 4) {
                    pixel[0] = v;
                    pixel[1] = v;
                    pixel[2] = v;
                    pixel[3] = 255;
                }
            });
        }
        x += scaled.h_advance(id);
    }
    export::encode_png(width, height, &rgba).unwrap()
}

fn newspaper_line() -> String {
    let pool = ["north", "south", "east", "west", "river", "field", "stone", "cloud"];
    let mut words = Vec::with_capacity(66);
    words.push("Aaron");
    for _ in 0..8 {
        words.extend(pool);
    }
    words.push("Zurich");
    words.join(" ")
}

/// The Latin recogniser reads both ends of a line that used to exceed its input, and the page
/// is not skipped. A second, ordinary line on the same page stays one piece.
#[test]
fn latin_ocr_reads_a_line_wider_than_the_recogniser() {
    if !ocr::available_for("la") {
        eprintln!("skipped: Latin OCR models not installed");
        return;
    }
    let long = newspaper_line();
    let png = wide_line_png(&long);
    let mut session = Session::new();
    let scan = session.create_from_images(&[("paper.png".into(), png)]).unwrap();
    let id = session.open("scan.pdf", None, scan, None).unwrap();
    let before = export::Exporter::new(session.get(id).unwrap()).png(0, 72.0).unwrap();
    let found = session.recognize_text(id, &[], OcrSettings { language: "la".into(), dpi: 600.0, ..OcrSettings::default() }).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].skipped.is_none(), "a line past the recogniser's widest input must not drop the page: {:?}", found[0].skipped);
    let text = found[0].text();
    let lower = text.to_lowercase();
    assert!(lower.contains("aaron"), "missing the start of the line in {text:?}");
    assert!(lower.contains("zurich"), "missing the end of the line in {text:?}");
    assert!(found[0].words.len() >= 2, "a line of more than 40 heights is read in pieces: {text:?}");
    for word in &found[0].words {
        let across = word.across[0].hypot(word.across[1]);
        let up = word.up[0].hypot(word.up[1]).max(1e-3);
        assert!(across / up <= 45.0, "piece {:?} is still wider than the recogniser reads", word.text);
    }
    let extracted = page_text(&session, id).to_lowercase();
    assert!(extracted.contains("aaron") && extracted.contains("zurich"), "search must hit both ends: {extracted:?}");
    assert_eq!(export::Exporter::new(session.get(id).unwrap()).png(0, 72.0).unwrap(), before, "search text changes no pixels");
    let saved = session.save_bytes(id).unwrap();
    let reopened = session.open("result.pdf", None, saved, None).unwrap();
    let reopened_text = page_text(&session, reopened).to_lowercase();
    assert!(reopened_text.contains("aaron") && reopened_text.contains("zurich"), "both ends survive save/reopen: {reopened_text:?}");
    session.undo(id).unwrap();
    assert!(page_text(&session, id).trim().is_empty(), "undo removes the recognised line");
}

fn page_text(session: &Session, id: pdfcraft_engine::DocId) -> String {
    let doc = session.get(id).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
    renderer
        .render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() })
        .text
        .map(|t| t.plain_text())
        .unwrap_or_default()
}
