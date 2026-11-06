//! Real automatic Edit entry, using the installed OCR models (same skip as the other OCR tests).
use super::*;

mod rectangles;

fn frame(h: &mut Harness<'_, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    h.state_mut().prepare_raw_input(&ctx, &mut input);
    *h.input_mut() = input;
    h.step();
}

fn enter_edit(h: &mut Harness<'_, PdfCraftApp>) {
    h.get_by_label("Edit a PDF").click();
    for _ in 0..4 {
        frame(h);
    }
    assert!(h.state().ocr_run.is_none(), "inline real-model worker finished");
}

#[test]
fn automatic_edit_leaves_rectangle_texture_clean_and_offers_explicit_ocr_once() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let shot = std::env::var("PDFCRAFT_AUTO_OCR_SHOT").ok();
    let _gpu = shot.as_ref().map(|_| crate::gpu());
    let original = rectangles::fixture();
    let mut h = edit_harness(original.clone());
    let id = h.state().active_ids().unwrap().1;
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
    enter_edit(&mut h);
    let doc = h.state().session.get(id).unwrap();
    assert!(!doc.dirty, "spurious rectangle words must not modify the PDF");
    assert_eq!(doc.can_undo(), None, "no Recognize text edit was applied");
    assert_eq!(doc.bytes.as_ref(), &original);
    assert!(doc.text_lines(0).is_empty());
    let notice = h.state().toast.as_ref().map(|t| t.0.as_str());
    assert_eq!(
        notice,
        Some(
            "Automatic text recognition was skipped for this page, so no editable text was added. To recognize it, use Recognize text in Scan & OCR."
        )
    );
    if let Some(path) = shot {
        h.render().unwrap().save(path).unwrap();
    }
    h.state_mut().toast = None;
    for _ in 0..8 {
        frame(&mut h);
    }
    assert!(h.state().toast.is_none(), "the page is neither retried nor notified every frame");
    assert!(!h.state().session.get(id).unwrap().dirty);
    // Explicit recognition still applies this same result: the automatic heuristic is not an
    // engine policy, and the user can override it through the action named in the notice.
    assert!(h.state_mut().execute("ocr.recognize"));
    frame(&mut h);
    frame(&mut h);
    h.get_by_label("Recognize text").click();
    for _ in 0..4 {
        frame(&mut h);
    }
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.dirty);
    assert_eq!(doc.can_undo(), Some("Recognize text"));
    assert!(!doc.text_lines(0).is_empty());
}

#[test]
fn automatic_edit_still_applies_a_single_hello_scan() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut session = Session::new();
    let blank = session.create_blank(612.0, 792.0, 1).unwrap();
    let id = session.open("original.pdf", None, blank, None).unwrap();
    let text = AddedText { rect: [72.0, 590.0, 540.0, 740.0], text: "HELLO".into(), size: 24.0, ..Default::default() };
    session.apply(id, Edit::AddText { page: 0, text }).unwrap();
    let png = export::Exporter::new(session.get(id).unwrap()).png(0, 150.0).unwrap();
    let scan = session.create_from_images_with_resolution(&[("scan.png".into(), png)], pdfcraft_engine::ImageResolution::Dpi(150.0)).unwrap();
    let mut h = edit_harness(scan.to_vec());
    let id = h.state().active_ids().unwrap().1;
    assert!(!h.state().session.get(id).unwrap().dirty);
    enter_edit(&mut h);
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.dirty, "a useful single-word scan is still recognized automatically");
    assert_eq!(doc.can_undo(), Some("Recognize text"));
    assert_eq!(doc.text_lines(0).iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["HELLO"]);
}

#[test]
fn automatic_edit_keeps_a_textless_picture_silent_and_clean() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::GrayImage::from_pixel(256, 256, image::Luma([245])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let original = Session::new().create_from_images(&[("blank.png".into(), png.into_inner())]).unwrap();
    let mut h = edit_harness(original.to_vec());
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().toast = None;
    enter_edit(&mut h);
    let doc = h.state().session.get(id).unwrap();
    assert!(!doc.dirty);
    assert_eq!(doc.can_undo(), None);
    assert_eq!(doc.bytes.as_ref(), original.as_ref());
    assert!(doc.text_lines(0).is_empty());
    assert!(h.state().toast.is_none(), "automatic OCR on a textless picture must stay silent");
}
