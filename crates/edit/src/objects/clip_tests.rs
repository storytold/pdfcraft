use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_incremental};

use super::*;

/// A document of `objs` (object 1 the catalog), numbered in order.
fn pdf(objs: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// Two 300 × 300 pages. The first draws `source` with Helvetica as /F1, a 4 × 2 RGB image /Im0,
/// a half-opaque ExtGState /GS0 and Courier as /F2; the second draws `target` with Courier
/// under the same name /F1, so a paste from the first must rename the font it brings.
fn two_pages(source: &str, target: &str) -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R /F2 8 0 R >> /XObject << /Im0 9 0 R >> /ExtGState << /GS0 << /ca 0.5 /CA 0.5 >> >> >> >>"
            .into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 8 0 R >> >> >>".into(),
        stream(source),
        stream(target),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 24 >>\nstream\n\u{7f}\u{0}\u{0}\u{0}\u{7f}\u{0}\u{0}\u{0}\u{7f}\u{7f}\u{7f}\u{0}\u{0}\u{7f}\u{7f}\u{7f}\u{0}\u{7f}@@@\u{0}\u{0}\u{0}\nendstream".into(),
    ])
}

/// What the page state, the copied paragraph and picture and the objects left out are drawn with.
const SOURCE: &str = "q 1 0 0 1 10 10 cm 0 0 270 270 re W n 0 0 1 rg /GS0 gs \
    BT /F2 10 Tf 200 20 Td (Skip) Tj ET \
    BT /F1 24 Tf 1.5 Tc 20 170 Td (Copied) Tj 0 -30 Td (Too) Tj ET \
    q 1 0 0 rg 150 230 100 40 re f Q \
    q 80 0 0 40 20 60 cm /Im0 Do Q Q \
    BT /F1 12 Tf 200 250 Td (After) Tj ET";

fn target(doc: &Document, page: usize, want: impl Fn(&EditableObject) -> bool) -> ObjectTarget {
    editable_objects(doc, page).unwrap().into_iter().find(|o| want(o)).unwrap().target
}

fn render(doc: &Document, page: usize) -> pdfcraft_render::RenderedPage {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    renderer.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Pixels, scale: 1.0, ..Default::default() })
}

/// The pixels of `page` in user-space box `r` (a page 300 high at scale 1), shifted by `by`.
fn pixels(page: &pdfcraft_render::RenderedPage, r: [f64; 4], by: [f64; 2]) -> Vec<u8> {
    let (x0, x1) = ((r[0] + by[0]) as u32, (r[2] + by[0]) as u32);
    let (y0, y1) = ((300.0 - r[3] - by[1]) as u32, (300.0 - r[1] - by[1]) as u32);
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * page.width + x) * 4) as usize;
            out.extend_from_slice(&page.rgba[i..i + 4]);
        }
    }
    out
}

/// The paragraph "Copied" / "Too" (one or two of them, as the lines group) and the picture.
fn copied_paragraph_and_picture(doc: &Document) -> CopiedObjects {
    let targets: Vec<_> = editable_objects(doc, 0)
        .unwrap()
        .into_iter()
        .filter(|o| o.target.kind == ObjectKind::Image || o.text.as_deref().is_some_and(|t| t.contains("Copied") || t.contains("Too")))
        .map(|o| o.target)
        .collect();
    copy_objects(doc, 0, &targets).unwrap()
}

#[test]
fn a_copy_takes_the_selected_objects_their_state_and_only_the_resources_they_use() {
    let doc = two_pages(SOURCE, "");
    let copied = copied_paragraph_and_picture(&doc);
    let content = String::from_utf8_lossy(&copied.content).into_owned();
    assert!(content.contains("(Copied)") && content.contains("(Too)") && content.contains("/Im0 Do"), "{content}");
    // The page's clip and its colour and transparency still apply; nothing else is painted.
    assert!(content.contains("W\nn") && content.contains("/GS0 gs") && content.contains("rg"), "{content}");
    assert!(!content.contains("Skip") && !content.contains("After") && !content.contains("\nf\n"), "{content}");
    let names = |cat: &[u8]| copied.resources.get(cat).and_then(Object::as_dict).map(|d| d.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>());
    assert_eq!(names(b"Font"), Some(vec![b"F1".to_vec(), b"F2".to_vec()]), "the font of the skipped text stays selected for what follows");
    assert_eq!(names(b"XObject"), Some(vec![b"Im0".to_vec()]));
    assert_eq!(names(b"ExtGState"), Some(vec![b"GS0".to_vec()]));
    assert!(copied.text().contains("Copied") && copied.text().contains("Too") && !copied.text().contains("Skip"), "{}", copied.text());
    assert_eq!(pdfcraft_content::parse(&copied.content).ops.iter().filter(|o| o.is("q")).count(), 3, "balanced: {content}");
}

#[test]
fn pasted_objects_draw_the_same_on_another_page_under_renamed_resources() {
    let mut doc = two_pages(SOURCE, "BT /F1 10 Tf 5 5 Td (x) Tj ET");
    let copied = copied_paragraph_and_picture(&doc);
    paste_objects(&mut doc, 1, &copied, [0.0, 0.0]).unwrap();
    let (source, pasted) = (render(&doc, 0), render(&doc, 1));
    let area = [copied.rect[0] - 4.0, copied.rect[1] - 4.0, copied.rect[2] + 4.0, copied.rect[3] + 4.0];
    let (before, after) = (pixels(&source, area, [0.0, 0.0]), pixels(&pasted, area, [0.0, 0.0]));
    assert!(before.chunks(4).any(|p| p != [255, 255, 255, 255]), "the copied objects are drawn");
    assert_eq!(before, after, "the paste draws the same pixels");
    let red = pixels(&pasted, [160.0, 240.0, 260.0, 280.0], [0.0, 0.0]);
    assert!(red.chunks(4).all(|p| p == [255, 255, 255, 255]), "what wasn't selected isn't pasted");
    // Its own /F1 (Courier) is left alone; the copy's Helvetica comes under another name.
    let fonts =
        pdfcraft_model::pages(&doc)[1].dict.get(b"Resources").map(|r| doc.resolve(r)).unwrap().as_dict().unwrap().get(b"Font").cloned().unwrap();
    let fonts = doc.resolve(&fonts).as_dict().cloned().unwrap();
    assert_eq!(fonts.get(b"F1"), Some(&Object::Ref(pdfcraft_cos::ObjRef::new(8, 0))));
    assert!(fonts.iter().any(|(k, v)| k != b"F1" && *v == Object::Ref(pdfcraft_cos::ObjRef::new(7, 0))), "{fonts:?}");
    let texts: Vec<_> = editable_objects(&doc, 1).unwrap().into_iter().filter_map(|o| o.text).collect();
    assert!(texts.iter().any(|t| t.contains("Copied")) && !texts.iter().any(|t| t.contains("Skip")), "{texts:?}");
    assert_eq!(crate::images::page_images(&doc, 1).unwrap().len(), 1);
}

#[test]
fn a_paste_onto_its_own_page_moves_by_the_offset_and_draws_the_same() {
    let mut doc = two_pages(SOURCE, "");
    let copied = copied_paragraph_and_picture(&doc);
    let before = render(&doc, 0);
    paste_objects(&mut doc, 0, &copied, [150.0, 0.0]).unwrap();
    let after = render(&doc, 0);
    let area = [copied.rect[0] - 2.0, copied.rect[1] - 2.0, copied.rect[2] + 2.0, copied.rect[3] + 2.0];
    assert!(area[2] + 150.0 < 300.0, "{area:?}");
    // The copy (and the clip it is drawn in) lands 150 pt to the right, the original stays.
    // Antialiasing elsewhere on the page may round an edge pixel one step differently.
    let (moved, original) = (pixels(&after, area, [150.0, 0.0]), pixels(&before, area, [0.0, 0.0]));
    assert_eq!(moved.len(), original.len());
    assert!(original.iter().any(|v| *v != 255), "the copied objects are drawn");
    assert!(moved.iter().zip(&original).all(|(a, b)| a.abs_diff(*b) <= 2), "the paste draws the same pixels, moved");
    assert_eq!(pixels(&after, area, [0.0, 0.0]), original);
    // Within the document and the page, the resources are the page's own, under their names.
    let res = pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").map(|r| doc.resolve(r)).unwrap();
    let fonts = res.as_dict().unwrap().get(b"Font").and_then(Object::as_dict).cloned().unwrap();
    assert_eq!(fonts.len(), 2, "{fonts:?}");
    assert_eq!(editable_objects(&doc, 0).unwrap().iter().filter(|o| o.target.kind == ObjectKind::Image).count(), 2);
}

/// Artwork without resources of its own takes the names of the page drawing it. Pasted onto a
/// page whose names differ, artwork that names nothing still draws the same; artwork that names
/// a font would find another one, so the paste is refused.
#[test]
fn artwork_inheriting_resources_pastes_only_when_it_names_none() {
    for (art, pastes) in [("0 0 1 rg 0 0 60 30 re f", true), ("BT /F1 9 Tf (n) Tj ET", false)] {
        let mut doc = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 300] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> /XObject << /Art 8 0 R >> >> >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> >>".into(),
            stream("BT /F1 12 Tf 20 250 Td (Title) Tj ET q 1 0 0 1 20 100 cm /Art Do Q"),
            stream(""),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            format!("<< /Type /XObject /Subtype /Form /BBox [0 0 60 30] /Length {} >>\nstream\n{art}\nendstream", art.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
        ]);
        let all: Vec<_> = editable_objects(&doc, 0).unwrap().into_iter().map(|o| o.target).collect();
        let copied = copy_objects(&doc, 0, &all).unwrap();
        let pasted = paste_objects(&mut doc, 1, &copied, [0.0, 0.0]);
        assert_eq!(pasted.is_ok(), pastes, "{art}: {pasted:?}");
        if let Err(EditError::Invalid(why)) = pasted {
            assert!(why.starts_with("the copied objects can't be pasted here") && why.contains("uses its resources"), "{why}");
        }
    }
}

#[test]
fn added_content_is_copied_with_everything_its_stream_paints() {
    let mut doc = two_pages(SOURCE, "");
    let item = crate::Content::Text(crate::AddedText {
        rect: [20.0, 20.0, 200.0, 60.0],
        text: "Added box".into(),
        size: 12.0,
        ..crate::AddedText::default()
    });
    crate::add_content(&mut doc, 0, &item).unwrap();
    let added = target(&doc, 0, |o| o.target.kind == ObjectKind::Added);
    let copied = copy_objects(&doc, 0, &[added]).unwrap();
    assert_eq!(copied.text(), "Added box");
    paste_objects(&mut doc, 1, &copied, [0.0, 0.0]).unwrap();
    let texts: Vec<_> = editable_objects(&doc, 1).unwrap().into_iter().filter_map(|o| o.text).collect();
    assert_eq!(texts, ["Added box"], "pasted as the page's own text");
    assert!(crate::list_added(&doc).iter().all(|a| a.page == 0));
}

#[test]
fn copying_and_pasting_refuse_what_they_cannot_do_and_change_nothing() {
    let doc = two_pages(SOURCE, "");
    let text = target(&doc, 0, |o| o.text.as_deref().is_some_and(|t| t.contains("Copied")));
    assert!(copy_objects(&doc, 0, &[]).is_err());
    assert!(copy_objects(&doc, 0, &[text, text]).is_err(), "selected twice");
    assert!(copy_objects(&doc, 0, &[ObjectTarget { kind: ObjectKind::Image, index: 9 }]).is_err(), "stale");
    assert!(copy_objects(&doc, 5, &[text]).is_err(), "no such page");
    let copied = copy_objects(&doc, 0, &[text]).unwrap();
    for offset in [[f64::NAN, 0.0], [f64::INFINITY, 0.0], [2e9, 0.0]] {
        let mut work = doc.clone();
        assert!(paste_objects(&mut work, 1, &copied, offset).is_err(), "{offset:?}");
        assert!(!work.is_modified(), "a refused paste changes nothing");
    }
    assert!(paste_objects(&mut doc.clone(), 7, &copied, [0.0, 0.0]).is_err());

    // Text drawn as a clipping path before the selection would clip it: refused, not dropped.
    let clipped = two_pages("BT /F1 20 Tf 7 Tr 20 20 Td (Mask) Tj ET 0 Tr BT /F1 20 Tf 20 200 Td (Shown) Tj ET", "");
    let shown = target(&clipped, 0, |o| o.text.as_deref() == Some("Shown"));
    assert!(copy_objects(&clipped, 0, &[shown]).is_err());

    // Stray `Q`, `ET` and an unfinished path or text object: the copy is still balanced.
    let stray = two_pages("Q Q ET 10 10 m 20 20 l BT /F1 20 Tf 20 200 Td (Stray) Tj", "");
    let shown = target(&stray, 0, |o| o.text.as_deref() == Some("Stray"));
    let copied = copy_objects(&stray, 0, &[shown]).unwrap();
    let ops = pdfcraft_content::parse(&copied.content).ops;
    assert_eq!(ops.iter().filter(|o| o.is("q")).count(), ops.iter().filter(|o| o.is("Q")).count());
    assert_eq!(ops.iter().filter(|o| o.is("BT")).count(), ops.iter().filter(|o| o.is("ET")).count());
    let mut work = stray.clone();
    paste_objects(&mut work, 1, &copied, [0.0, 0.0]).unwrap();
}
