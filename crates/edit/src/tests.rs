use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full, write_incremental};

use super::*;

/// Three pages: an upright page with content that leaves the graphics state changed (an
/// unbalanced `cm`), a page rotated 90° and one with inherited, shared resources.
fn fixture() -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 600 800] /Resources 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents [7 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Font << /F1 8 0 R >> >>".into(),
        "<< /Length 37 >>\nstream\n2 0 0 2 0 0 cm BT /F1 9 Tf (Hi) Tj ET\nendstream".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
    ];
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

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

/// The decoded content streams of a page, in order.
fn streams(doc: &Document, page: usize) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[page];
    let c = p.dict.get(b"Contents").cloned();
    let list = match c.map(|c| doc.resolve(&c)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(_) => vec![p.dict.get(b"Contents").cloned().unwrap()],
        None => vec![],
    };
    list.iter().map(|o| String::from_utf8_lossy(&stream_bytes(doc, o).unwrap()).into_owned()).collect()
}

fn cx() -> Context {
    Context { date: (2026, 10, 1) }
}

#[test]
fn an_image_turns_by_any_angle_about_its_centre_and_saves_the_matrix() {
    // 100,100..300,200 is 200 x 100 around (200, 150).
    let mut doc = image_page();
    let r0 = images::page_images(&doc, 0).unwrap()[0].rect;
    let t = images::turn_by_degrees(r0, 30.0).unwrap();
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let doc = reopen(&doc);
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let (w, h) = (200.0 * c + 100.0 * s, 200.0 * s + 100.0 * c);
    assert!(close(r, [200.0 - w / 2.0, 150.0 - h / 2.0, 200.0 + w / 2.0, 150.0 + h / 2.0]), "{r:?}");
    // 90 degrees is the quarter turn; whole turns and non-finite angles are harmless.
    let q = images::turn_about_centre(r0, 1, false, false);
    let d = images::turn_by_degrees(r0, 90.0).unwrap();
    assert!(q.iter().zip(&d).all(|(a, b)| (a - b).abs() < 1e-9), "{q:?} {d:?}");
    let full = images::turn_by_degrees(r0, 720.0).unwrap();
    assert!(full.iter().zip([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).all(|(a, b)| (a - b).abs() < 1e-9));
    assert!(images::turn_by_degrees(r0, f64::NAN).is_none() && images::turn_by_degrees(r0, f64::INFINITY).is_none());
}

#[test]
fn an_added_image_crop_keeps_some_of_each_axis() {
    use crate::added::AddedImage;
    assert!(AddedImage::crop_valid([0.7, 0.0, 0.0, 0.95]));
    assert!(AddedImage::crop_valid([0.5, 0.45, 0.45, 0.5]));
    // 0.05 + 0.9 rounds to just above 0.95: still the 5 % kept, accepted and left as it is.
    assert!(AddedImage::crop_valid([0.05, 0.0, 0.9, 0.0]));
    assert_eq!(AddedImage::clamp_crop([0.05, 0.0, 0.9, 0.0]), [0.05, 0.0, 0.9, 0.0]);
    assert!(!AddedImage::crop_valid([0.06, 0.0, 0.9, 0.0]), "more than 95 % across");
    assert!(!AddedImage::crop_valid([0.6, 0.0, 0.4, 0.0]), "nothing left across");
    assert!(!AddedImage::crop_valid([-0.1, 0.0, 0.0, 0.0]));
    assert!(!AddedImage::crop_valid([f64::NAN, 0.0, 0.0, 0.0]));
    assert_eq!(AddedImage::clamp_crop([0.7, 0.0, 0.0, 0.0]), [0.7, 0.0, 0.0, 0.0], "a valid crop stays as it is");
    // From a file: too much across keeps the two sides' proportion; nonsense trims nothing.
    let [l, b, r, t] = AddedImage::clamp_crop([0.6, f64::INFINITY, 0.6, -1.0]);
    assert!((l - 0.475).abs() < 1e-12 && (r - 0.475).abs() < 1e-12 && b == 0.0 && t == 0.0);
    // Sides beyond the whole image keep their proportion too, and huge ones don't overflow.
    let [l, _, r, _] = AddedImage::clamp_crop([1.5, 0.0, 0.5, 0.0]);
    assert!((l - 0.7125).abs() < 1e-12 && (r - 0.2375).abs() < 1e-12, "{l} {r}");
    let [l, _, r, _] = AddedImage::clamp_crop([f64::MAX, 0.0, f64::MAX, 0.0]);
    assert!((l - 0.475).abs() < 1e-12 && (r - 0.475).abs() < 1e-12, "{l} {r}");
    let [l, _, r, _] = AddedImage::clamp_crop([1.5, 0.0, 0.0, 0.0]);
    assert!((l - 0.95).abs() < 1e-12 && r == 0.0, "one side alone is capped at the limit");
}

#[test]
fn tokens_expand() {
    let c = cx();
    assert_eq!(expand("Page <<1>> of <<n>>", 3, 10, 0, &c), "Page 3 of 10");
    assert_eq!(expand("<<1 of n>> · <<Page 1>> · <<1/n>>", 2, 5, 0, &c), "2 of 5 · Page 2 · 2/5");
    assert_eq!(expand("<<m/d/yyyy>> <<yyyy-mm-dd>> <<mmmm d, yyyy>>", 1, 1, 0, &c), "10/1/2026 2026-10-01 October 1, 2026");
    assert_eq!(expand("<<yyyy/mm/dd>> <<yyyy/m/d>>", 1, 1, 0, &c), "2026/10/01 2026/10/1");
    assert_eq!(expand("<<dd.mm.yyyy>> <<d.m.yyyy>> <<dd.mm.yy>> <<d.m.yy>>", 1, 1, 0, &c), "01.10.2026 1.10.2026 01.10.26 1.10.26");
    assert_eq!(expand("<<Bates Number#6#100#ABC#-X>>", 1, 1, 104, &c), "ABC000104-X");
    assert_eq!(expand("keep <<unknown>> and <<unclosed", 1, 1, 0, &c), "keep <<unknown>> and <<unclosed");
    assert_eq!(bates_start("x <<Bates Number#6#100#A#B>>"), Some(100));
}

#[test]
fn header_and_footer_are_drawn_in_display_space_and_wrap_the_original_content() {
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: ["Left".into(), String::new(), "<<Page 1 of n>>".into(), String::new(), "Confidential (draft)".into(), String::new()],
        ..HeaderFooter::default()
    };
    add_header_footer(&mut doc, &[0, 1, 2], &hf, false, &cx()).unwrap();
    let doc = reopen(&doc);
    let s0 = streams(&doc, 0);
    // q-wrapper, original, Q-wrapper, header/footer.
    assert_eq!(s0.len(), 4, "{s0:?}");
    // The close starts on a new line, so a comment the original ends in can't swallow it.
    assert_eq!((s0[0].as_str(), s0[2].as_str()), ("q %PdfCraft\n", "\nQ %PdfCraft\n"));
    let mark = &s0[3];
    assert!(mark.contains("/PCMark /HeaderFooter") && mark.contains("(Page 1 of 3) Tj") && mark.contains("(Confidential \\(draft\\)) Tj"), "{mark}");
    assert!(mark.contains("1 0 0 1 0 0 cm"), "upright page: identity");
    // The rotated page maps display space onto its rotated crop box.
    let s1 = streams(&doc, 1);
    assert!(s1.last().unwrap().contains("0 1 -1 0 600 0 cm") && s1.last().unwrap().contains("(Page 2 of 3)"), "{s1:?}");
    // A page without content gets just the mark; its inherited resources are copied, not changed.
    assert_eq!(streams(&doc, 2).len(), 1);
    let p2 = &pdfcraft_model::pages(&doc)[2];
    let res = doc.resolve(p2.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    assert!(fonts.as_dict().unwrap().contains(b"PCHelv") && fonts.as_dict().unwrap().contains(b"F1"));
    let shared = doc.get(pdfcraft_cos::ObjRef::new(6, 0));
    let shared_fonts = doc.resolve(shared.as_dict().unwrap().get(b"Font").unwrap());
    assert!(!shared_fonts.as_dict().unwrap().contains(b"PCHelv"), "the shared dictionary is untouched");
    assert_eq!(marks_present(&doc), [MarkKind::HeaderFooter]);
}

#[test]
fn header_and_footer_are_drawn_in_the_chosen_standard_face() {
    // #3754: Times or Courier instead of Helvetica. The content names the face, the page's fonts
    // gain it, and right-aligned text is placed by that face's widths (not Helvetica's).
    let text = "Wide words";
    for (font, resource, base) in [(added::Family::Times, "PCTimes", "Times-Roman"), (added::Family::Courier, "PCCour", "Courier")] {
        let mut doc = fixture();
        let hf = HeaderFooter {
            text: [String::new(), String::new(), text.into(), String::new(), String::new(), String::new()],
            font,
            ..HeaderFooter::default()
        };
        add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
        let doc = reopen(&doc);
        let mark = streams(&doc, 0).last().cloned().unwrap();
        assert!(mark.contains(&format!("/{resource} 8 Tf")), "{base}: {mark}");
        let x = 600.0 - 72.0 - font.width(text, 8.0, false, false);
        assert!((x - (600.0 - 72.0 - pdfcraft_fonts::helvetica_width(text, 8.0))).abs() > 1.0, "{base}: the faces differ in width");
        assert!(mark.contains(&format!("1 0 0 1 {} ", n(x))), "{base}: right-aligned by its own widths: {mark}");
        let page = &pdfcraft_model::pages(&doc)[0];
        let res = doc.resolve(page.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().cloned().unwrap();
        let face = doc.resolve(fonts.get(resource.as_bytes()).unwrap()).as_dict().cloned().unwrap();
        assert_eq!(face.name(b"BaseFont"), Some(base.as_bytes()), "{base}");
    }
    // The default is still Helvetica, and only Helvetica is added.
    let mut doc = fixture();
    let hf =
        HeaderFooter { text: [text.into(), String::new(), String::new(), String::new(), String::new(), String::new()], ..HeaderFooter::default() };
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    let mark = streams(&reopen(&doc), 0).last().cloned().unwrap();
    assert!(mark.contains("/PCHelv 8 Tf") && !mark.contains("/PCTimes") && !mark.contains("/PCCour"), "{mark}");
}

#[test]
fn a_header_never_takes_over_a_font_name_the_page_already_draws_with() {
    // From review of #3756: a page that already draws a different font as /PCTimes keeps it; the
    // header's Times goes under a free name, and an update reuses that name, not a third one.
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let mut fonts = Dict::new();
    let mut theirs = Dict::new();
    theirs.set(b"Type".to_vec(), Object::name("Font"));
    theirs.set(b"Subtype".to_vec(), Object::name("Type1"));
    theirs.set(b"BaseFont".to_vec(), Object::name("Helvetica-Bold"));
    fonts.set(b"PCTimes".to_vec(), Object::Dict(theirs));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
    let hf = HeaderFooter {
        text: ["Serif".into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        font: added::Family::Times,
        ..HeaderFooter::default()
    };
    let base_of = |doc: &Document, name: &[u8]| {
        let fonts = page_fonts(doc, &pdfcraft_model::pages(doc)[0]);
        fonts.get(name).map(|f| doc.resolve(f)).and_then(|f| f.as_dict().and_then(|d| d.name(b"BaseFont")).map(<[u8]>::to_vec))
    };
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    let doc = reopen(&doc);
    assert_eq!(base_of(&doc, b"PCTimes").as_deref(), Some(&b"Helvetica-Bold"[..]), "the page's own font is untouched");
    assert_eq!(base_of(&doc, b"PCTimes_1").as_deref(), Some(&b"Times-Roman"[..]));
    let mark = streams(&doc, 0).last().cloned().unwrap();
    assert!(mark.contains("/PCTimes_1 8 Tf"), "{mark}");
    // Updating the header finds its own name again.
    let mut doc = doc;
    add_header_footer(&mut doc, &[0], &hf, true, &cx()).unwrap();
    let doc = reopen(&doc);
    assert!(streams(&doc, 0).last().unwrap().contains("/PCTimes_1 8 Tf"));
    assert_eq!(base_of(&doc, b"PCTimes_2"), None, "no new name on update");
    assert_eq!(base_of(&doc, b"PCTimes").as_deref(), Some(&b"Helvetica-Bold"[..]));
    // The same face with more to it (here a /ToUnicode map) is the page's own too: kept whole.
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let map = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), b"/CIDInit /ProcSet findresource begin")));
    let mut theirs = Dict::new();
    for (k, v) in [("Type", "Font"), ("Subtype", "Type1"), ("BaseFont", "Times-Roman"), ("Encoding", "WinAnsiEncoding")] {
        theirs.set(k.as_bytes().to_vec(), Object::name(v));
    }
    theirs.set(b"ToUnicode".to_vec(), Object::Ref(map));
    let mut fonts = Dict::new();
    fonts.set(b"PCTimes".to_vec(), Object::Dict(theirs));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    let doc = reopen(&doc);
    let fonts = page_fonts(&doc, &pdfcraft_model::pages(&doc)[0]);
    let kept = doc.resolve(fonts.get(b"PCTimes").unwrap()).as_dict().cloned().unwrap();
    assert!(kept.contains(b"ToUnicode"), "the page's map is kept");
    assert!(streams(&doc, 0).last().unwrap().contains("/PCTimes_1 8 Tf"));
}

#[test]
fn header_start_number_that_would_wrap_is_refused_without_drawing() {
    // u32::MAX is a valid start_number on its own, but there is no room for a second page
    // number after it. The old code wrapped the second page to zero; now the whole run is
    // refused and the document keeps no header at all.
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: [String::new(), String::new(), "<<1>>".into(), String::new(), String::new(), String::new()],
        start_number: u32::MAX,
        ..HeaderFooter::default()
    };
    let err = add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap_err();
    assert!(err.to_string().contains("leaves no room"), "{err}");
    assert!(marks_present(&doc).is_empty(), "nothing is drawn when the run is refused");
    // A single page still fits the start number itself.
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    assert_eq!(marks_present(&doc).len(), 1);
}

#[test]
fn header_bates_start_that_would_overflow_is_refused_without_drawing() {
    // A Bates start of u64::MAX has no room for a second page. The run must be refused up front
    // instead of overflowing (a panic in debug builds, a wrap to 0 in release builds).
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: [String::new(), String::new(), "<<Bates Number#6#18446744073709551615#A#B>>".into(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    let err = add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap_err();
    assert!(err.to_string().contains("Bates"), "{err}");
    assert!(marks_present(&doc).is_empty(), "nothing is drawn when the run is refused");
    // A single page still fits the start number itself.
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    assert_eq!(marks_present(&doc).len(), 1);
}

#[test]
fn replace_and_remove_restore_the_original_content() {
    let mut doc = fixture();
    let original = streams(&doc, 0);
    let mut hf = HeaderFooter::default();
    hf.text[1] = "First".into();
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    hf.text[1] = "Second".into();
    add_header_footer(&mut doc, &[0], &hf, true, &cx()).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.iter().filter(|x| x.contains("/PCMark")).count(), 1, "replaced, not added");
    assert!(s.last().unwrap().contains("(Second)"));
    add_watermark(&mut doc, &[0], &Watermark { text: "DRAFT".into(), ..Watermark::default() }, false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [1.0, 1.0, 0.9], opacity: 1.0, ..Background::default() }, false).unwrap();
    assert_eq!(marks_present(&doc).len(), 3);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Watermark).unwrap(), 1);
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Background"), "the background stays behind: {s:?}");
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Background).unwrap(), 1);
    assert_eq!(streams(&reopen(&doc), 0), original, "back to exactly the original content");
}

/// The six numbers of the first `… cm` in `content`.
fn first_cm(content: &str) -> [f64; 6] {
    let line = content.lines().find(|l| l.ends_with(" cm")).unwrap_or_else(|| panic!("no cm in {content:?}"));
    let v: Vec<f64> = line.split_whitespace().take(6).map(|x| x.parse().unwrap()).collect();
    <[f64; 6]>::try_from(v).unwrap()
}

fn map_point(m: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

fn shrinking(header: &str, footer: &str) -> HeaderFooter {
    let mut hf = HeaderFooter { shrink: true, ..HeaderFooter::default() };
    hf.text[1] = header.into();
    hf.text[5] = footer.into();
    hf
}

#[test]
fn shrink_scales_the_original_content_into_the_band_between_header_and_footer() {
    let mut doc = fixture();
    // 8 pt text, margins 36 top and bottom: the header takes 36 + 8 (+ 4 clearance) at the top,
    // the footer 36 + 6.4 (its capitals above the baseline, + 4) at the bottom.
    add_header_footer(&mut doc, &[0], &shrinking("ACME", "<<Bates Number#6#1##>>"), false, &cx()).unwrap();
    let doc = reopen(&doc);
    let s = streams(&doc, 0);
    // q-wrapper, shrink, original, Q-wrapper, header/footer.
    assert_eq!(s.len(), 5, "{s:?}");
    assert_eq!((s[0].as_str(), s[3].as_str()), ("q %PdfCraft\n", "\nQ %PdfCraft\n"));
    assert!(s[2].contains("(Hi) Tj"), "the original content follows the shrink: {s:?}");
    let m = first_cm(&s[1]);
    let k = (800.0 - 48.0 - 46.4) / 800.0;
    assert!((m[0] - k).abs() < 1e-3 && (m[3] - k).abs() < 1e-3 && m[1] == 0.0 && m[2] == 0.0, "uniform scale {k}: {m:?}");
    // The page's corners land on the band, centred across.
    assert!(near(map_point(m, 0.0, 0.0), ((600.0 - 600.0 * k) / 2.0, 46.4)), "{m:?}");
    assert!(near(map_point(m, 600.0, 800.0), ((600.0 + 600.0 * k) / 2.0, 800.0 - 48.0)), "{m:?}");
    assert!(s[4].contains("(000001) Tj"), "{}", s[4]);
}

#[test]
fn shrink_follows_the_display_space_of_a_rotated_page() {
    let mut doc = fixture();
    // Page 2 is rotated 90°: shown 800 wide and 600 high. A footer alone keeps the top.
    add_header_footer(&mut doc, &[1], &shrinking("", "<<1>>"), false, &cx()).unwrap();
    let s = streams(&doc, 1);
    let m = first_cm(&s[1]);
    let page = &pdfcraft_model::pages(&doc)[1];
    let v = page.view_matrix(&doc);
    let k = (600.0 - 46.4) / 600.0;
    // Display (x, y) is user v·(x, y): the shrunk content's display corners.
    let to_user = |x: f64, y: f64| map_point(v, x, y);
    assert!(near(map_point(m, to_user(0.0, 0.0).0, to_user(0.0, 0.0).1), to_user((800.0 - 800.0 * k) / 2.0, 46.4)), "{m:?}");
    assert!(near(map_point(m, to_user(800.0, 600.0).0, to_user(800.0, 600.0).1), to_user((800.0 + 800.0 * k) / 2.0, 600.0)), "{m:?}");
}

#[test]
fn shrink_is_undone_by_remove_and_not_doubled_by_update() {
    let mut doc = fixture();
    let original = streams(&doc, 0);
    add_background(&mut doc, &[0], &Background::default(), false).unwrap();
    add_header_footer(&mut doc, &[0], &shrinking("First", ""), false, &cx()).unwrap();
    add_header_footer(&mut doc, &[0], &shrinking("Second", ""), true, &cx()).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.iter().filter(|x| x.contains(" cm") && !x.contains("/PCMark")).count(), 2, "one shrink besides the original's own cm: {s:?}");
    assert!(s[0].contains("/PCMark /Background"), "the background stays behind, full size: {s:?}");
    // Update without shrinking: back to full size.
    let mut plain = shrinking("Third", "");
    plain.shrink = false;
    add_header_footer(&mut doc, &[0], &plain, true, &cx()).unwrap();
    assert_eq!(streams(&doc, 0).len(), 5, "background, wrapper, original, wrapper, header: {:?}", streams(&doc, 0));
    add_header_footer(&mut doc, &[0], &shrinking("Fourth", ""), true, &cx()).unwrap();
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 2, "the header and its shrink");
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Background).unwrap(), 1);
    assert_eq!(streams(&reopen(&doc), 0), original, "back to exactly the original content");
}

#[test]
fn shrink_with_no_room_left_is_refused_without_drawing() {
    let mut doc = fixture();
    let mut hf = shrinking("Top", "Bottom");
    hf.margins = [400.0, 400.0, 72.0, 72.0];
    let err = add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap_err();
    assert!(err.to_string().contains("no room"), "{err}");
    assert!(!doc.is_modified());
    // A page without content has nothing to shrink: it just gets the header.
    let mut doc = fixture();
    add_header_footer(&mut doc, &[2], &shrinking("Top", ""), false, &cx()).unwrap();
    assert_eq!(streams(&doc, 2).len(), 1);
}

#[test]
fn watermarks_rotate_fade_and_can_go_behind() {
    let mut doc = fixture();
    let wm = Watermark { text: "CONFIDENTIAL\nDo not copy".into(), opacity: 0.3, rotation: 45.0, behind: true, ..Watermark::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Watermark"), "behind: first");
    assert!(s[0].contains("/PCGS30 gs") && s[0].contains("0.707 0.707 -0.707 0.707 300 400 cm"), "{}", s[0]);
    assert!(s[0].contains("(CONFIDENTIAL) Tj") && s[0].contains("(Do not copy) Tj"));
    assert_eq!(s.len(), 2, "no wrapper needed for content behind");
}

#[test]
fn a_second_background_is_drawn_over_the_first() {
    // #3591: the new background went in front of the old one in /Contents, so the old colour
    // was painted over it and stayed.
    let mut doc = fixture();
    add_background(&mut doc, &[0], &Background { color: [0.0, 0.0, 1.0], opacity: 1.0, ..Background::default() }, false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [0.0, 1.0, 0.0], opacity: 1.0, ..Background::default() }, false).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.len(), 3, "{s:?}");
    assert!(s[0].contains("0 0 1 rg") && s[1].contains("0 1 0 rg"), "the newer background is painted last: {s:?}");
    assert!(s[2].contains("(Hi) Tj"), "both stay behind the content: {s:?}");
}

#[test]
fn backgrounds_stay_below_watermarks_behind_the_content() {
    // Behind the content, backgrounds come first, in the order added, and then watermarks, so a
    // watermark added after a background shows and a later background doesn't cover it.
    let mut doc = fixture();
    add_background(&mut doc, &[0], &Background { color: [0.0, 0.0, 1.0], opacity: 1.0, ..Background::default() }, false).unwrap();
    add_watermark(&mut doc, &[0], &Watermark { text: "DRAFT".into(), behind: true, ..Watermark::default() }, false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [0.0, 1.0, 0.0], opacity: 1.0, ..Background::default() }, false).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.len(), 4, "{s:?}");
    assert!(s[0].contains("0 0 1 rg") && s[1].contains("0 1 0 rg"), "backgrounds first, oldest first: {s:?}");
    assert!(s[2].contains("/PCMark /Watermark") && s[3].contains("(Hi) Tj"), "then the watermark, then the content: {s:?}");
}

#[test]
fn a_watermark_behind_goes_under_one_on_top_of_a_page_without_content() {
    // With no content of its own, the page's watermark on top is its first stream, which looks
    // like a behind-mark; a watermark added behind later must still be painted under it.
    let mut doc = fixture();
    add_watermark(&mut doc, &[2], &Watermark { text: "TOP".into(), ..Watermark::default() }, false).unwrap();
    add_watermark(&mut doc, &[2], &Watermark { text: "BEHIND".into(), behind: true, ..Watermark::default() }, false).unwrap();
    let s = streams(&doc, 2);
    assert_eq!(s.len(), 2, "{s:?}");
    assert!(s[0].contains("(BEHIND) Tj") && s[1].contains("(TOP) Tj"), "the watermark behind is painted first: {s:?}");
}

#[test]
fn watermark_offset_moves_the_centre_right_and_up() {
    let mut doc = fixture();
    let wm = Watermark { text: "DRAFT".into(), rotation: 0.0, offset: [-100.0, 50.0], ..Watermark::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let s = streams(&doc, 0);
    let mark = s.iter().find(|c| c.contains("/PCMark /Watermark")).expect("the watermark stream");
    assert!(mark.contains("1 0 0 1 200 450 cm"), "{mark}");
}

#[test]
fn a_watermark_offset_that_is_not_finite_is_refused() {
    for offset in [[f64::NAN, 0.0], [0.0, f64::INFINITY], [f64::NEG_INFINITY, 0.0]] {
        let mut doc = fixture();
        let wm = Watermark { text: "DRAFT".into(), offset, ..Watermark::default() };
        assert!(matches!(add_watermark(&mut doc, &[0], &wm, false), Err(EditError::Invalid(_))), "{offset:?}");
        assert!(!doc.is_modified());
    }
}

#[test]
fn invalid_requests_change_nothing() {
    let mut doc = fixture();
    assert!(matches!(add_header_footer(&mut doc, &[0], &HeaderFooter::default(), false, &cx()), Err(EditError::Invalid(_))));
    assert!(matches!(add_watermark(&mut doc, &[0], &Watermark::default(), false), Err(EditError::Invalid(_))));
    assert_eq!(
        add_background(&mut doc, &[9], &Background { color: [1.0; 3], opacity: 1.0, ..Background::default() }, false),
        Err(EditError::NoSuchPage(9))
    );
    assert_eq!(EditError::NoSuchPage(9).to_string(), "page 10 does not exist", "the message counts pages from 1 (#1550)");
    assert!(!doc.is_modified());
}

#[test]
fn flattening_draws_appearances_into_the_page_and_removes_the_comments() {
    use pdfcraft_annot::{Meta, NewAnnotation, NoteIcon, Shape, Style, add_annotation, add_reply};
    let mut doc = fixture();
    let meta = Meta { date: None, id: "x".into() };
    let add = |doc: &mut Document, shape: Shape| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: "c".into(), author: "a".into() }, &meta).unwrap()
    };
    add(&mut doc, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] });
    let note = add(&mut doc, Shape::Note { at: [200.0, 700.0], icon: NoteIcon::Comment });
    add_reply(&mut doc, 0, note, "reply", "b", &meta).unwrap();
    let before = streams(&doc, 0);
    let n = flatten(&mut doc, &[0], true, false).unwrap();
    assert_eq!(n, 2, "the rectangle and the note icon are drawn; the reply has nothing to draw");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(!p.dict.contains(b"Annots"), "comments, pop-up and reply are gone");
    let s = streams(&doc, 0);
    assert_eq!(s.len(), before.len() + 3, "wrapped original + flattened content: {s:?}");
    let flat = s.last().unwrap();
    assert!(flat.contains("/PCFl0 Do") && flat.contains("/PCFl1 Do"), "{flat}");
    // The rectangle's appearance is drawn at its rectangle (bbox = rect, identity mapping).
    assert!(flat.contains("q 1 0 0 1 0 0 cm /PCFl0 Do Q"), "{flat}");
    // The note's 20×20 icon box is mapped onto its rect at (200, 680).
    assert!(flat.contains("1 0 0 1 200 680 cm /PCFl1 Do"), "{flat}");
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    assert!(xo.as_dict().unwrap().contains(b"PCFl1"));
    assert!(marks_present(&doc).is_empty(), "flattened content is not a removable mark");
}

/// #2368: a widget with /NoRotate on a rotated page is shown upright, turned with the page's
/// /Rotate about its rectangle's upper-left corner (§12.5.3, as MuPDF and Poppler draw it).
/// Flattening drew its appearance at the rectangle as it is, so the widget turned sideways and
/// moved; a neighbour without the flag is unchanged, and so is everything on an unrotated page.
#[test]
fn flattening_keeps_a_no_rotate_widget_upright_on_a_rotated_page() {
    let page = stream("", " ");
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] /Length 15 >>\nstream\n0 0 100 50 re f\nendstream";
    for (rotate, plain, upright) in [
        (0, "q 1 0 0 1 100 500 cm /PCFl0 Do Q", "q 1 0 0 1 100 500 cm /PCFl1 Do Q"),
        (90, "q 1 0 0 1 100 500 cm /PCFl0 Do Q", "q 0 1 -1 0 150 550 cm /PCFl1 Do Q"),
        (180, "q 1 0 0 1 100 500 cm /PCFl0 Do Q", "q -1 0 0 -1 100 600 cm /PCFl1 Do Q"),
        (270, "q 1 0 0 1 100 500 cm /PCFl0 Do Q", "q 0 -1 1 0 50 550 cm /PCFl1 Do Q"),
    ] {
        let widget =
            |flags: i64| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f{flags}) /Rect [100 500 200 550] /F {flags} /AP << /N 7 0 R >> >>");
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Rotate {rotate} /Resources << >> /Contents 6 0 R /Annots [4 0 R 5 0 R] >>"
            ),
            &widget(4),
            &widget(4 | 16),
            &page,
            form,
        ]);
        assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 2);
        let doc = reopen(&doc);
        let s = streams(&doc, 0);
        let flat = s.last().unwrap();
        assert!(flat.contains(plain), "/Rotate {rotate}: {flat}");
        assert!(flat.contains(upright), "/Rotate {rotate}: {flat}");
    }
}

/// #1317: a cloudy square this builder cannot draw stays on the page. A solid square, a hidden
/// cloudy square and a cloudy square that already has an appearance keep their previous results.
#[test]
fn flattening_keeps_a_cloudy_square_it_cannot_draw() {
    let page = stream("", " ");
    let form = "<< /Type /XObject /Subtype /Form /BBox [100 600 200 650] /Length 28 >>\nstream\n1 0 0 RG 100 600 100 50 re S\nendstream";
    let case = |extra: &str, drawn: usize, left: bool| {
        let annot = format!("<< /Type /Annot /Subtype /Square /Rect [100 600 200 650] /C [1 0 0] /Contents (Synthetic comment) {extra} >>");
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 5 0 R /Annots [4 0 R] >>",
            &annot,
            &page,
            form,
        ]);
        let before = streams(&doc, 0);
        assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), drawn);
        let doc = reopen(&doc);
        let p = &pdfcraft_model::pages(&doc)[0];
        assert_eq!(p.dict.contains(b"Annots"), left);
        if drawn == 0 {
            assert_eq!(streams(&doc, 0), before, "nothing was drawn in place of the comment");
        }
    };
    case("/BE << /S /C /I 2 >>", 0, true);
    case("/BE << /S /S >>", 1, false);
    case("/BE << /S /C /I 2 >> /F 2", 0, true);
    case("/BE << /S /C /I 2 >> /AP << /N 6 0 R >>", 1, false);
}

/// #805: a supported note with no stored appearance is still drawn by viewers. Flattening must
/// generate that appearance and bake it into the page, not delete a comment that shows an icon.
#[test]
fn flattening_keeps_a_note_whose_appearance_is_missing() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (Synthetic note) /Name /Comment /Open false /F 0 >>",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1, "the note icon is drawn");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(!p.dict.contains(b"Annots"), "the note is gone once its icon is baked in");
    let flat = streams(&doc, 0).pop().unwrap();
    assert!(flat.contains("q 1 0 0 1 10 10 cm /PCFl0 Do Q"), "{flat}");
}

/// #3175: a note without an appearance is drawn as a fixed-size icon at the upper-left corner of
/// its `/Rect`. A rectangle with no width (LaTeX `pdfcomment`) used to scale the icon to nothing
/// while flattening still counted it as drawn and deleted the note.
#[test]
fn flattening_a_note_with_a_zero_width_rect_draws_a_fixed_size_icon() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Text /Rect [50 60 50 72] /Contents (Zero width) /Name /Comment /F 4 >>",
        "<< /Type /Annot /Subtype /Text /Rect [10 20 150 120] /Contents (Large) /F 4 >>",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 2, "both icons are drawn");
    let doc = reopen(&doc);
    let flat = streams(&doc, 0).pop().unwrap();
    assert!(flat.contains("q 1 0 0 1 50 52 cm /PCFl0 Do Q"), "{flat}");
    assert!(flat.contains("q 1 0 0 1 10 100 cm /PCFl1 Do Q"), "{flat}");
}

/// #1470: a form field whose widget is on no page (left behind by page extraction or combining)
/// has nothing to draw, but flattening every form field still takes it out of `/AcroForm /Fields`,
/// or other software keeps treating the flattened file as a form. A hidden widget on a page is
/// left alone, as before.
#[test]
fn flattening_fields_drops_the_ones_on_no_page() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (on_page) /Rect [10 10 150 40] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (orphan) /Rect [10 50 150 80] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hidden) /Rect [10 90 150 120] /F 2 /AP << /N 10 0 R >> >>",
        "<< /FT /Tx /T (parent) /Kids [9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [10 130 150 160] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "only the field on the page is drawn");
    let doc = reopen(&doc);
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(af.as_dict().unwrap().get(b"Fields").unwrap());
    let left: Vec<pdfcraft_cos::ObjRef> = fields.as_array().unwrap().iter().filter_map(|f| f.as_ref()).collect();
    assert_eq!(left, [pdfcraft_cos::ObjRef::new(7, 0)], "only the hidden field on the page stays");
}

/// #1794: a form field whose widget has no usable appearance (none stored, or none for its
/// selected state) has nothing to draw. Flattening used to remove it anyway, losing its value
/// with no ink in its place. It stays, in the page and in the form, like a comment that can't be
/// drawn; a field with an appearance is still baked in and removed.
#[test]
fn flattening_keeps_a_valued_widget_it_cannot_draw() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 10 150 40] /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Yes /AS /Yes /Rect [10 50 30 70] /F 4 /AP << /N << /Off 8 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (drawn) /V (Bob) /Rect [10 90 150 120] /F 4 /AP << /N 8 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "only the field with an appearance is drawn");
    let doc = reopen(&doc);
    let refs = |o: &Object| -> Vec<u32> { doc.resolve(o).as_array().unwrap().iter().filter_map(|f| f.as_ref()).map(|r| r.num).collect() };
    let p = &pdfcraft_model::pages(&doc)[0];
    assert_eq!(refs(p.dict.get(b"Annots").unwrap()), [5, 6], "the undrawable widgets stay on the page");
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    assert_eq!(refs(af.as_dict().unwrap().get(b"Fields").unwrap()), [5, 6], "and in the form");
    let value = |n: u32| doc.get(pdfcraft_cos::ObjRef::new(n, 0)).as_dict().unwrap().get(b"V").cloned();
    assert_eq!(value(5), Some(Object::String(pdfcraft_cos::PdfString::literal(b"Ada".to_vec()))));
    assert_eq!(value(6), Some(Object::name("Yes")));
}

/// #4017: a widget with no appearance whose field has no value (none, or an empty string) has
/// nothing to lose, so flattening removes it instead of leaving a fillable field behind. One whose
/// value it inherits from its parent field still stays (#1794).
#[test]
fn flattening_removes_an_empty_widget_it_cannot_draw() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R 9 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (no_value) /Rect [10 10 150 40] /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (empty) /V () /Rect [10 50 150 80] /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (drawn) /Rect [10 90 150 120] /F 4 /AP << /N 10 0 R >> >>",
        "<< /FT /Tx /T (parent) /V (Ada) /Kids [9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [10 130 150 160] /F 4 >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "only the field with an appearance is drawn");
    let doc = reopen(&doc);
    let refs = |o: &Object| -> Vec<u32> { doc.resolve(o).as_array().unwrap().iter().filter_map(|f| f.as_ref()).map(|r| r.num).collect() };
    let p = &pdfcraft_model::pages(&doc)[0];
    assert_eq!(refs(p.dict.get(b"Annots").unwrap()), [9], "only the widget with an inherited value stays on the page");
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    assert_eq!(refs(af.as_dict().unwrap().get(b"Fields").unwrap()), [8], "and its field in the form");
}

/// #4017: when the walk up the parent fields reaches its limit before finding a value, the widget
/// stays: an unverified value must not be removed. Here `/V` sits on the 64th ancestor.
#[test]
fn flattening_keeps_a_widget_whose_value_is_past_the_parent_limit() {
    let page = stream("", " ");
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [69 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>".into(),
        page,
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [10 10 150 40] /F 4 >>".into(),
    ];
    objs.extend((6..69).map(|n| format!("<< /Parent {} 0 R /Kids [{} 0 R] >>", n + 1, n - 1)));
    objs.push("<< /FT /Tx /T (deep) /V (Ada) /Kids [68 0 R] >>".into());
    let mut doc = build(&objs.iter().map(String::as_str).collect::<Vec<_>>());
    flatten(&mut doc, &[0], false, true).unwrap();
    let doc = reopen(&doc);
    let annots = pdfcraft_model::pages(&doc)[0].dict.get(b"Annots").map(|a| doc.resolve(a).as_array().map_or(0, Vec::len));
    assert_eq!(annots, Some(1), "the widget stays");
}

/// #1794: a reply this builder cannot draw stays on the page even when the comment it replies to
/// is flattened; the comment's pop-up still goes with it.
#[test]
fn flattening_keeps_an_undrawn_reply_to_a_flattened_comment() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Square /Rect [100 600 200 650] /C [1 0 0] /Contents (Synthetic comment) /AP << /N 8 0 R >> /Popup 7 0 R >>",
        "<< /Type /Annot /Subtype /Square /Rect [100 500 200 550] /C [1 0 0] /Contents (Synthetic reply) /BE << /S /C /I 2 >> /IRT 5 0 R >>",
        "<< /Type /Annot /Subtype /Popup /Rect [300 600 400 700] /Parent 5 0 R >>",
        "<< /Type /XObject /Subtype /Form /BBox [100 600 200 650] /Length 28 >>\nstream\n1 0 0 RG 100 600 100 50 re S\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1, "the comment is drawn; the cloudy reply cannot be");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    let left: Vec<u32> = doc.resolve(p.dict.get(b"Annots").unwrap()).as_array().unwrap().iter().filter_map(|f| f.as_ref()).map(|r| r.num).collect();
    assert_eq!(left, [6], "the undrawn reply stays; the comment and its pop-up are gone");
}

/// Flattening never makes a `?` permanent: it refuses, changing nothing, a text box without an
/// appearance whose text Helvetica can't draw (another program's), and a comment or form field
/// whose appearance PdfCraft drew with `?` (marked `PCUndrawn`). A comment another program drew
/// with a font that has those characters (an appearance, no mark) flattens as before.
#[test]
fn flattening_refuses_what_would_bake_in_question_marks() {
    let page = stream("", " ");
    let refused = |annot: &str, comments: bool, fields: bool, what: &str| {
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
            &page,
            annot,
            "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
        ]);
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let err = flatten(&mut doc, &[0], comments, fields).unwrap_err().to_string();
        assert!(err.contains(what) && err.contains("page 1") && err.contains("\"欢\" (U+6B22)") && err.contains("nothing was flattened"), "{err}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before, "a refused flatten changes nothing");
        assert!(pdfcraft_model::pages(&doc)[0].dict.contains(b"Annots"), "the annotation is still there");
    };
    refused(
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 >>",
        true,
        false,
        "a comment",
    );
    refused(
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 /AP << /N 6 0 R >> /PCUndrawn <FEFF6B22> >>",
        true,
        false,
        "a comment",
    );
    refused(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V <FEFF6B228FCE> /Rect [10 10 150 40] /F 4 /AP << /N 6 0 R >> /PCUndrawn <FEFF6B22> >>",
        false,
        true,
        "a form field",
    );
    // Drawn by another program, with a font that has 欢: no mark, so it flattens.
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 /AP << /N 6 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1);
}

/// Display space is in points, so on a `/UserUnit 2` page an item's box maps to half as many
/// user-space units. Items record the `/UserUnit` they were written for; one without it was
/// written when display space was in user-space units, and reads back scaled to points.
#[test]
fn added_items_follow_the_page_user_unit() {
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[2].obj;
    doc.update_dict(page, |d| d.set(b"UserUnit".to_vec(), Object::Int(2))).unwrap();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Scaled".into(), size: 14.0, ..AddedText::default() };
    add_content(&mut doc, 2, &Content::Text(text.clone())).unwrap();
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    let doc = reopen(&doc);
    let all = list_added(&doc);
    let on = |page: usize| all.iter().find(|a| a.page == page).unwrap();
    let Content::Text(t) = &on(2).content else { panic!() };
    assert_eq!((t.rect[0], t.rect[2], t.rect[3], t.size), (72.0, 300.0, 700.0, 14.0), "read back as written");
    assert!(streams(&doc, 2).iter().any(|s| s.starts_with("q 0.5 0 0 0.5 0 0 cm")), "display points to user space");
    let recorded = |doc: &Document, a: &Added| {
        doc.get(a.obj).as_dict().and_then(|d| d.get(b"PCAdded").cloned()).and_then(|p| p.as_dict().and_then(|p| p.get(b"UserUnit").cloned()))
    };
    assert!(recorded(&doc, on(0)).is_none(), "a page without /UserUnit records nothing");
    assert_eq!(recorded(&doc, on(2)).and_then(|u| u.as_f64()), Some(2.0));
    // The same item as an earlier version wrote it: no record, box and size in user-space units.
    let mut legacy = doc.clone();
    let Object::Stream(mut s) = (*legacy.get(on(2).obj)).clone() else { panic!() };
    let mut params = s.dict.get(b"PCAdded").and_then(|p| p.as_dict().cloned()).unwrap();
    params.remove(b"UserUnit");
    s.dict.set(b"PCAdded".to_vec(), Object::Dict(params));
    legacy.set(on(2).obj, Object::Stream(s));
    let Content::Text(old) = &list_added(&legacy).into_iter().find(|a| a.page == 2).unwrap().content else { panic!() };
    assert_eq!((old.rect[0], old.rect[2], old.rect[3], old.size), (144.0, 600.0, 1400.0, 28.0), "scaled to points");
}

fn tagged_dictionary(tag: &str, added: Option<Object>) -> Dict {
    let mut dict = Dict::new();
    dict.set(b"PCMark".to_vec(), Object::name(tag));
    if let Some(added) = added {
        dict.set(b"PCAdded".to_vec(), added);
    }
    dict
}

fn tagged_stream(doc: &mut Document, tag: &str, added: Option<Object>) -> Object {
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged_dictionary(tag, added), Vec::new()))))
}

fn added_text_params() -> Dict {
    let mut params = Dict::new();
    params.set(b"Rect".to_vec(), Object::Array([10, 20, 110, 40].into_iter().map(Object::Int).collect()));
    params.set(b"Kind".to_vec(), Object::name("Text"));
    params.set(b"Text".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Added text")));
    params.set(b"Size".to_vec(), Object::Int(12));
    params
}

fn added_image_params(image: pdfcraft_cos::ObjRef) -> Dict {
    let mut params = Dict::new();
    params.set(b"Rect".to_vec(), Object::Array([10, 20, 110, 70].into_iter().map(Object::Int).collect()));
    params.set(b"Kind".to_vec(), Object::name("Image"));
    params.set(b"Image".to_vec(), Object::Ref(image));
    params
}

fn set_page_contents(doc: &mut Document, contents: Object, user_unit: Option<Object>) {
    let page = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page, |dict| {
        dict.set(b"Contents".to_vec(), contents);
        if let Some(user_unit) = user_unit {
            dict.set(b"UserUnit".to_vec(), user_unit);
        }
    })
    .unwrap();
}

fn assert_marker_scan_matches_legacy(doc: &Document, label: &str, marks: &[MarkKind], added: usize) -> MarkerScan {
    let expected_marks = marks_present(doc);
    let expected_added = list_added(doc);
    let scan = scan_markers(doc);
    assert_eq!(scan.marks, expected_marks, "{label}: marks differ from the independent scan");
    assert_eq!(scan.added, expected_added, "{label}: added content differs from the independent scan");
    assert_eq!(scan.marks, marks, "{label}: fixture's mark order or deduplication is wrong");
    assert_eq!(scan.added.len(), added, "{label}: fixture's added-item count is wrong");
    scan
}

#[test]
fn combined_marker_scan_matches_the_independent_scans() {
    let empty = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"]);
    assert_marker_scan_matches_legacy(&empty, "empty page", &[], 0);
    assert_marker_scan_matches_legacy(&fixture(), "ordinary streams and a page without contents", &[], 0);

    for (tag, kind) in [("HeaderFooter", MarkKind::HeaderFooter), ("Watermark", MarkKind::Watermark), ("Background", MarkKind::Background)] {
        let mut doc = fixture();
        let marker = tagged_stream(&mut doc, tag, None);
        set_page_contents(&mut doc, marker, None);
        assert_marker_scan_matches_legacy(&doc, tag, &[kind], 0);
    }

    let mut all_marks = fixture();
    let background = tagged_stream(&mut all_marks, "Background", None);
    let watermark = tagged_stream(&mut all_marks, "Watermark", None);
    let header = tagged_stream(&mut all_marks, "HeaderFooter", None);
    set_page_contents(
        &mut all_marks,
        Object::Array(vec![background.clone(), watermark.clone(), header.clone(), watermark, header, background]),
        None,
    );
    assert_marker_scan_matches_legacy(&all_marks, "all mark kinds repeated", &[MarkKind::Background, MarkKind::Watermark, MarkKind::HeaderFooter], 0);

    let mut single_reference = fixture();
    let marker = tagged_stream(&mut single_reference, "Watermark", None);
    set_page_contents(&mut single_reference, marker, None);
    assert_marker_scan_matches_legacy(&single_reference, "contents as one reference", &[MarkKind::Watermark], 0);

    let mut text_only = fixture();
    let text = tagged_stream(&mut text_only, "Added", Some(Object::Dict(added_text_params())));
    set_page_contents(&mut text_only, text, None);
    let text_scan = assert_marker_scan_matches_legacy(&text_only, "one added text item", &[], 1);
    assert!(matches!(&text_scan.added[0].content, Content::Text(text) if text.text == "Added text"));

    let mut indirect_params = fixture();
    let params = indirect_params.add(Object::Dict(added_text_params()));
    let text = tagged_stream(&mut indirect_params, "Added", Some(Object::Ref(params)));
    set_page_contents(&mut indirect_params, text, None);
    let indirect_scan = assert_marker_scan_matches_legacy(&indirect_params, "indirect PCAdded dictionary", &[], 1);
    assert!(matches!(&indirect_scan.added[0].content, Content::Text(text) if text.text == "Added text"));

    let mut image_only = fixture();
    let image = image_only.add(Object::Stream(Stream::from_raw(Dict::new(), vec![0])));
    let image_item = tagged_stream(&mut image_only, "Added", Some(Object::Dict(added_image_params(image))));
    set_page_contents(&mut image_only, image_item, None);
    let image_scan = assert_marker_scan_matches_legacy(&image_only, "one added image item", &[], 1);
    assert!(matches!(&image_scan.added[0].content, Content::Image(item) if item.image == image));

    let mut interleaved = fixture();
    let text = tagged_stream(&mut interleaved, "Added", Some(Object::Dict(added_text_params())));
    let watermark = tagged_stream(&mut interleaved, "Watermark", None);
    let image = interleaved.add(Object::Stream(Stream::from_raw(Dict::new(), vec![0])));
    let image_item = tagged_stream(&mut interleaved, "Added", Some(Object::Dict(added_image_params(image))));
    let header = tagged_stream(&mut interleaved, "HeaderFooter", None);
    set_page_contents(&mut interleaved, Object::Array(vec![text, watermark, image_item, header]), None);
    let interleaved_scan =
        assert_marker_scan_matches_legacy(&interleaved, "marks and added content interleaved", &[MarkKind::Watermark, MarkKind::HeaderFooter], 2);
    assert!(matches!(&interleaved_scan.added[0].content, Content::Text(_)));
    assert!(matches!(&interleaved_scan.added[1].content, Content::Image(_)));

    let mut array_contents = fixture();
    let background = tagged_stream(&mut array_contents, "Background", None);
    let array_ref = array_contents.add(Object::Array(vec![background]));
    set_page_contents(&mut array_contents, Object::Ref(array_ref), None);
    assert_marker_scan_matches_legacy(&array_contents, "contents as an indirect array", &[MarkKind::Background], 0);

    let mut malformed = fixture();
    let indirect_mark = malformed.add(Object::Dict(tagged_dictionary("HeaderFooter", None)));
    let missing_added = tagged_stream(&mut malformed, "Added", None);
    let invalid_added = tagged_stream(&mut malformed, "Added", Some(Object::String(pdfcraft_cos::PdfString::text("bad params"))));
    let unknown = tagged_stream(&mut malformed, "FutureKind", None);
    let direct_watermark = Object::Dict(tagged_dictionary("Watermark", None));
    let direct_added = Object::Dict(tagged_dictionary("Added", Some(Object::Dict(added_text_params()))));
    set_page_contents(
        &mut malformed,
        Object::Array(vec![
            Object::Null,
            Object::Int(7),
            Object::name("not-a-stream"),
            direct_watermark,
            Object::Ref(indirect_mark),
            missing_added,
            invalid_added,
            unknown,
            direct_added,
        ]),
        None,
    );
    assert_marker_scan_matches_legacy(
        &malformed,
        "malformed, non-stream, unknown and direct entries",
        &[MarkKind::Watermark, MarkKind::HeaderFooter],
        0,
    );

    let mut shared = fixture();
    let added = tagged_stream(&mut shared, "Added", Some(Object::Dict(added_text_params())));
    let mark = tagged_stream(&mut shared, "Background", None);
    set_page_contents(&mut shared, Object::Array(vec![added.clone(), mark.clone(), added.clone(), mark]), None);
    let shared_scan = assert_marker_scan_matches_legacy(&shared, "repeated shared stream references", &[MarkKind::Background], 2);
    assert_eq!(shared_scan.added[0].obj, shared_scan.added[1].obj);

    let mut user_unit = fixture();
    let added = tagged_stream(&mut user_unit, "Added", Some(Object::Dict(added_text_params())));
    set_page_contents(&mut user_unit, added, Some(Object::Int(2)));
    let unit_scan = assert_marker_scan_matches_legacy(&user_unit, "non-1.0 UserUnit", &[], 1);
    let Content::Text(text) = &unit_scan.added[0].content else { panic!() };
    assert_eq!(text.rect, [20.0, 40.0, 220.0, 80.0]);
    assert_eq!(text.size, 24.0);
}

#[test]
fn added_text_and_images_are_page_content_that_stays_editable() {
    let mut doc = fixture();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Approved by Ada\nSecond line".into(), size: 14.0, ..AddedText::default() };
    assert_eq!(add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap(), 0);
    // An 1×1 gray image, placed on the rotated page.
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(1));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let img = doc.add(Object::Stream(Stream::from_raw(d, vec![128])));
    add_content(&mut doc, 1, &Content::Image(AddedImage::new([10.0, 10.0, 110.0, 60.0], img))).unwrap();
    let doc2 = reopen(&doc);
    let all = list_added(&doc2);
    assert_eq!(all.len(), 2);
    let Content::Text(t) = &all[0].content else { panic!() };
    assert_eq!((t.text.as_str(), t.size), ("Approved by Ada\nSecond line", 14.0));
    assert_eq!(t.rect, [72.0, 700.0 - 2.0 * 14.0 * 1.2, 300.0, 700.0], "the box height follows the two lines");
    let page0 = streams(&doc2, 0).join("\n");
    assert!(page0.contains("(Approved by Ada) Tj") && page0.contains("(Second line) Tj") && page0.contains("/PCFHelvetica 14 Tf"), "{page0}");
    // The rotated page draws in display space: the view matrix comes first.
    let page1 = streams(&doc2, 1).join("\n");
    assert!(page1.contains("q 0 1 -1 0 600 0 cm") && page1.contains(&format!("/PCImg{} Do", img.num)), "{page1}");
    // Edit: move, restyle, retype; then delete.
    let mut doc = doc2;
    let moved = AddedText {
        rect: [100.0, 500.0, 300.0, 520.0],
        text: "Approved".into(),
        bold: true,
        family: Family::Times,
        align: Align::Right,
        color: [1.0, 0.0, 0.0],
        ..text
    };
    update_content(&mut doc, 0, 0, &Content::Text(moved)).unwrap();
    let page0 = streams(&doc, 0).join("\n");
    assert!(page0.contains("/PCFTimesBold 14 Tf 1 0 0 rg") && !page0.contains("Second line"), "{page0}");
    assert!(
        update_content(&mut doc, 1, 0, &Content::Text(AddedText { text: "x".into(), rect: [0.0, 0.0, 50.0, 10.0], ..AddedText::default() })).is_err(),
        "kinds don't change"
    );
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(list_added(&doc).len(), 1);
    assert!(!streams(&doc, 0).join("").contains("Approved"));
    assert!(add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 0.0, 100.0, 10.0], ..AddedText::default() })).is_err(), "empty text");
    // Page marks ignore added items.
    assert!(marks_present(&doc).is_empty());
}

/// #4464: a page left with one added item draws that item and nothing else.
#[test]
fn an_isolated_item_is_all_that_is_left_on_its_page() {
    let mut doc = fixture();
    let first = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "First".into(), size: 14.0, ..AddedText::default() };
    let second = AddedText { rect: [72.0, 400.0, 300.0, 500.0], text: "Second".into(), ..first.clone() };
    add_content(&mut doc, 0, &Content::Text(first)).unwrap();
    add_content(&mut doc, 0, &Content::Text(second)).unwrap();
    let before = streams(&doc, 0).join("\n");
    assert!(before.contains("(First) Tj") && before.contains("(Second) Tj"), "{before}");
    assert!(isolate_content(&mut doc, 0, 2).is_err(), "no third item");
    assert!(isolate_content(&mut doc, 9, 0).is_err(), "no such page");
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| d.set(b"Annots".to_vec(), Object::Array(Vec::new()))).unwrap();
    isolate_content(&mut doc, 0, 1).unwrap();
    let after = streams(&doc, 0).join("\n");
    assert!(after.contains("(Second) Tj") && !after.contains("(First) Tj"), "{after}");
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    assert!(page.dict.get(b"Annots").is_none(), "no annotations are drawn over it");
    assert_eq!(list_added(&doc).len(), 1);
}

/// Edit ▸ Paste (#2636): where a pasted item lands.
#[test]
fn a_paste_lands_where_the_copy_was_offset_per_paste_and_stays_on_the_page() {
    use crate::added::{PASTE_STEP, paste_rect};
    let page = [300.0, 400.0];
    let rect = [40.0, 300.0, 140.0, 320.0];
    assert_eq!(paste_rect(rect, page, 0, None), rect, "in place");
    let d = 2.0 * PASTE_STEP;
    assert_eq!(paste_rect(rect, page, 2, None), [40.0 + d, 300.0 - d, 140.0 + d, 320.0 - d], "two steps down and right");
    assert_eq!(paste_rect(rect, page, 1, Some([10.0, 50.0])), [10.0, 30.0, 110.0, 50.0], "its top-left corner at the point");
    assert_eq!(paste_rect(rect, page, 0, Some([290.0, 5.0])), [200.0, 0.0, 300.0, 20.0], "pulled back onto the page");
    assert_eq!(paste_rect([0.0, 0.0, 500.0, 600.0], page, 0, Some([10.0, 10.0])), [0.0, -200.0, 500.0, 400.0], "too large: left and top on the page");
    assert_eq!(paste_rect([140.0, 320.0, 40.0, 300.0], page, 0, None), rect, "a reversed box is normalized");
    assert_eq!(paste_rect(rect, [f64::NAN, 400.0], 1, None), rect, "a page without a size leaves the box alone");
    assert_eq!(paste_rect(rect, page, 1, Some([f64::INFINITY, 5.0])), rect, "so does a point that isn't a number");
    assert_eq!(paste_rect(rect, page, u32::MAX, None), [200.0, 0.0, 300.0, 20.0], "many pastes stop at the page's corner");
}

/// A paste steps on past exact copies of the item (its original, earlier pastes), and only those.
#[test]
fn a_paste_steps_past_copies_of_itself_only() {
    use crate::added::{PASTE_STEP, paste_place};
    let page = [300.0, 400.0];
    let text = |rect, t: &str| Content::Text(AddedText { rect, text: t.into(), ..AddedText::default() });
    let item = text([40.0, 300.0, 140.0, 314.4], "Note");
    let step = |n: f64| [40.0 + n * PASTE_STEP, 300.0 - n * PASTE_STEP, 140.0 + n * PASTE_STEP, 314.4 - n * PASTE_STEP];
    assert_eq!(paste_place([], &item, page), step(0.0), "an empty page: where it was");
    assert_eq!(paste_place([&item], &item, page), step(1.0), "beside its original");
    let first = item.with_rect(step(1.0));
    assert_eq!(paste_place([&item, &first], &item, page), step(2.0), "beside the last copy");
    assert_eq!(paste_place([&first], &item, page), step(0.0), "its place is free again (the original was cut)");
    let other = text(step(0.0), "Other text");
    assert_eq!(paste_place([&other], &item, page), step(0.0), "a different item in the way doesn't count");
    // A page full of copies at its corner: the search ends.
    let corner = item.with_rect([200.0, 0.0, 300.0, 14.4]);
    let stuck = paste_place([&corner], &item.with_rect([200.0, 0.0, 300.0, 14.4]), page);
    assert_eq!(stuck, [200.0, 0.0, 300.0, 14.4]);
    // An image pasted into another document draws its picture from an object of that document:
    // still a copy, so the next paste steps past it.
    let image = Content::Image(AddedImage { crop: [0.1, 0.0, 0.0, 0.0], ..AddedImage::new(step(0.0), pdfcraft_cos::ObjRef::new(7, 0)) });
    let pasted = Content::Image(AddedImage { crop: [0.1, 0.0, 0.0, 0.0], ..AddedImage::new(step(0.0), pdfcraft_cos::ObjRef::new(42, 0)) });
    assert_eq!(paste_place([&pasted], &image, page), step(1.0), "a copy whose picture is stored anew");
    let turned = Content::Image(AddedImage { rotation: 1, ..AddedImage::new(step(0.0), pdfcraft_cos::ObjRef::new(7, 0)) });
    assert_eq!(paste_place([&turned], &image, page), step(0.0), "the same picture turned or cropped otherwise isn't a copy");
}

#[test]
fn added_arabic_text_is_shaped_in_display_order_and_stays_searchable() {
    let mut doc = fixture();
    let text =
        AddedText { rect: [72.0, 600.0, 400.0, 700.0], text: "مرحبا World".into(), size: 14.0, align: Align::Right, ..AddedText::default() };
    if pdfcraft_fonts::document_arabic_font().is_none() {
        eprintln!("skipping the Arabic drawing checks: built without a craft-fonts Arab face (set CRAFT_FONTS_DIR)");
        let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err();
        assert!(matches!(&err, EditError::Invalid(m) if m.contains("CRAFT_FONTS_DIR")), "{err:?}");
        assert!(!doc.is_modified(), "nothing is written when the font is missing");
        return;
    }
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // A second Arabic item on the same page keeps its own fonts.
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "شكرا".into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() })).unwrap();
    let doc = reopen(&doc);
    let added = list_added(&doc);
    let Content::Text(t) = &added[0].content else { panic!() };
    assert_eq!(t.text, "مرحبا World", "the logical text is kept for editing");
    let s = streams(&doc, 0);
    let item = s.iter().find(|s| s.contains("(World) Tj")).unwrap();
    // A right-to-left paragraph: "World" is drawn left of the Arabic word.
    let x_before = |s: &str, at: usize| -> f64 {
        let tm = s[..at].rfind("1 0 0 1 ").unwrap() + 8;
        s[tm..].split(' ').next().unwrap().parse().unwrap()
    };
    let latin_x = x_before(item, item.find("(World) Tj").unwrap());
    let arabic_at = item.find("/PCAr").unwrap();
    let arabic_x = x_before(item, item[arabic_at..].find(" Tj").unwrap() + arabic_at);
    assert!(latin_x < arabic_x, "{item}");
    // The Arabic glyphs come from Type 3 fonts whose ToUnicode maps give back each letter.
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone();
    let arabic: Vec<_> = fonts.iter().filter(|(k, _)| k.starts_with(b"PCAr")).map(|(_, v)| doc.resolve(v).as_dict().unwrap().clone()).collect();
    assert_eq!(arabic.len(), 2, "one font per item");
    let mut mapped = String::new();
    for f in &arabic {
        assert_eq!(f.name(b"Subtype"), Some(&b"Type3"[..]));
        mapped += &String::from_utf8_lossy(&stream_bytes(&doc, f.get(b"ToUnicode").unwrap()).unwrap());
    }
    for letter in ["0645", "0631", "062D", "0628", "0627", "0634", "0643"] {
        assert!(mapped.contains(&format!("<{letter}>")), "{letter} in {mapped}");
    }
}

#[test]
fn added_hebrew_text_is_drawn_right_to_left_and_stays_searchable() {
    let mut doc = fixture();
    let text = AddedText {
        rect: [72.0, 600.0, 400.0, 700.0],
        text: "שלום (עולם) 2026 World".into(),
        size: 14.0,
        align: Align::Right,
        ..AddedText::default()
    };
    if pdfcraft_fonts::document_hebrew_font(false).is_none() {
        eprintln!("skipping the Hebrew drawing checks: built without a craft-fonts Hebr face (set CRAFT_FONTS_DIR)");
        assert_eq!(first_undrawable(&text), Some('ש'));
        let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err();
        assert!(matches!(&err, EditError::Invalid(m) if m.contains("U+05E9")), "{err:?}");
        assert!(!doc.is_modified(), "nothing is written when the font is missing");
        return;
    }
    assert_eq!(first_undrawable(&text), None);
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // Points stay on their letters; a Hebrew line in an Arabic item and Arabic in a Hebrew one.
    for other in ["שָׁלוֹם", "שלום مرحبا"] {
        let other = AddedText { text: other.into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() };
        if other.text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)) && pdfcraft_fonts::document_arabic_font().is_none() {
            continue;
        }
        add_content(&mut doc, 0, &Content::Text(other)).unwrap();
    }
    let doc = reopen(&doc);
    let added = list_added(&doc);
    let Content::Text(t) = &added[0].content else { panic!() };
    assert_eq!(t.text, "שלום (עולם) 2026 World", "the logical text is kept for editing");
    let s = streams(&doc, 0);
    let item = s.iter().find(|s| s.contains("(World) Tj")).unwrap();
    // A right-to-left paragraph: "World", typed last, is drawn left of the Hebrew.
    let x_before = |s: &str, at: usize| -> f64 {
        let tm = s[..at].rfind("1 0 0 1 ").unwrap() + 8;
        s[tm..].split(' ').next().unwrap().parse().unwrap()
    };
    let latin_x = x_before(item, item.find("(World) Tj").unwrap());
    let hebrew_at = item.find("/PCAr").unwrap();
    let hebrew_x = x_before(item, item[hebrew_at..].find(" Tj").unwrap() + hebrew_at);
    assert!(latin_x < hebrew_x, "{item}");
    // Copy and search get the typing order back.
    let logical: String = t.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
    assert!(item.contains(&format!("/ActualText <FEFF{logical}>")), "{item}");
    // The Hebrew glyphs come from a Type 3 font named after the face, whose ToUnicode map gives
    // back each letter.
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone();
    let shaped: Vec<_> = fonts.iter().filter(|(k, _)| k.starts_with(b"PCAr")).map(|(_, v)| doc.resolve(v).as_dict().unwrap().clone()).collect();
    let mut mapped = String::new();
    for f in &shaped {
        assert_eq!(f.name(b"Subtype"), Some(&b"Type3"[..]));
        mapped += &String::from_utf8_lossy(&stream_bytes(&doc, f.get(b"ToUnicode").unwrap()).unwrap());
    }
    for letter in ["05E9", "05DC", "05D5", "05DD", "05E2"] {
        assert!(mapped.contains(&format!("<{letter}>")), "{letter} in {mapped}");
    }
    let font_name = |f: &Dict| doc.resolve(f.get(b"FontDescriptor").unwrap()).as_dict().unwrap().name(b"FontName").map(<[u8]>::to_vec);
    assert!(shaped.iter().any(|f| font_name(f).as_deref() == Some(&b"NotoSansHebrew"[..])));
    // A pointed letter is one glyph with its points, mapped back whole.
    assert!(mapped.contains("<05E905"), "{mapped}");
}

#[test]
fn odd_hebrew_text_never_panics() {
    let mut doc = fixture();
    let long = "שלום עולם ".repeat(2_000);
    for s in ["\u{202E}ש\u{05B8}\u{05B8} (]", "\u{05B8}", "א\u{200F}\u{2067}b\u{2069}", "א\n\nב\tc", "׳״ 456", long.as_str()] {
        let r = add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: s.into(), ..AddedText::default() }));
        assert_eq!(r.is_ok(), pdfcraft_fonts::document_hebrew_font(false).is_some(), "{:?}: {r:?}", s.chars().take(12).collect::<String>());
    }
}

/// The `PCAr` font names in page `page`'s resources.
fn arabic_font_names(doc: &Document, page: usize) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[page];
    let Some(res) = p.dict.get(b"Resources").map(|r| doc.resolve(r)) else { return vec![] };
    let Some(fonts) = res.as_dict().and_then(|r| r.get(b"Font")).map(|f| doc.resolve(f)) else { return vec![] };
    fonts.as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).filter(|k| k.starts_with("PCAr")).collect()
}

#[test]
fn arabic_items_keep_their_own_fonts_through_saves_updates_and_deletes() {
    let mut doc = fixture();
    let text = AddedText { rect: [72.0, 600.0, 400.0, 700.0], text: "مرحبا".into(), size: 14.0, ..AddedText::default() };
    if pdfcraft_fonts::document_arabic_font().is_none() {
        // An item that already holds Arabic (stored by an older version, drawn as "?") can still
        // be moved and retyped without the face.
        add_content(&mut doc, 0, &Content::Text(AddedText { text: "x".into(), ..text.clone() })).unwrap();
        update_content(&mut doc, 0, 0, &Content::Text(text)).unwrap();
        assert!(streams(&doc, 0).join("").contains("(?????) Tj"));
        return;
    }
    // A full save renumbers objects but keeps resource names, so a font named after its object
    // number may meet a name already on the page: here, every name it could take.
    let mut taken = Dict::new();
    for num in 1..2_000 {
        taken.set(format!("PCAr{num}").into_bytes(), Object::name("Taken"));
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(taken));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "شكرا".into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() })).unwrap();
    let names = arabic_font_names(&doc, 0);
    assert_eq!(names.len(), 1_999 + 2, "nothing replaced");
    let p = &pdfcraft_model::pages(&doc)[0];
    let fonts = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().unwrap().get(b"Font").cloned().unwrap();
    let fonts = doc.resolve(&fonts).as_dict().unwrap().clone();
    assert_eq!(fonts.iter().filter(|(_, v)| v.as_name() == Some(&b"Taken"[..])).count(), 1_999);
    let first = own(&doc, 0);
    assert!(first.len() == 1 && first[0].contains('_'), "a free name, not a taken one: {first:?}");
    // Updating an item swaps its fonts; deleting it removes them.
    update_content(&mut doc, 0, 0, &Content::Text(AddedText { text: "أهلا".into(), ..text })).unwrap();
    let names = arabic_font_names(&doc, 0);
    assert!(names.len() == 2_001 && !names.contains(&first[0]), "the replaced item's font is gone");
    delete_content(&mut doc, 0, 0).unwrap();
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(arabic_font_names(&doc, 0).len(), 1_999, "only the page's own names are left");
    reopen(&doc);
}

/// The `PCAr` fonts the stream of added item `index` on page 0 shows text with.
fn own(doc: &Document, index: usize) -> Vec<String> {
    let s = String::from_utf8_lossy(&stream_bytes(doc, &Object::Ref(list_added(doc)[index].obj)).unwrap()).into_owned();
    let mut names: Vec<String> =
        s.split_whitespace().filter_map(|w| w.strip_prefix('/')).filter(|w| w.starts_with("PCAr")).map(String::from).collect();
    names.dedup();
    names
}

#[test]
fn odd_arabic_text_never_panics() {
    let mut doc = fixture();
    let long = "بسم الله ".repeat(2_000);
    for s in ["\u{202E}ب\u{064B}\u{064B} (]", "\u{064B}", "ا\u{200F}\u{2067}b\u{2069}", "ا\n\nب\tc", "ﷺ ١٢٣ 456", long.as_str()] {
        let r = add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: s.into(), ..AddedText::default() }));
        assert_eq!(r.is_ok(), pdfcraft_fonts::document_arabic_font().is_some(), "{:?}: {r:?}", s.chars().take(12).collect::<String>());
    }
    // U+2029 and U+0085 (paragraph and line separators) have no WinAnsi code, so the standard font
    // would draw them as `?`: refused, by name, rather than written (#125). Laying it out still
    // mustn't panic.
    let r = add_content(
        &mut doc,
        0,
        &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: "ا\u{2029}ب\rc\u{85}د\n".into(), ..AddedText::default() }),
    );
    assert!(r.is_err(), "{r:?}");
    if pdfcraft_fonts::document_arabic_font().is_some() {
        assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("U+2029") || m.contains("U+0085")), "{r:?}");
    }
    // A character the face lacks is an error that names it, not a box or a crash.
    if pdfcraft_fonts::document_arabic_font().is_some() && !pdfcraft_fonts::arabic_has('\u{FDFD}') {
        let r = add_content(
            &mut doc,
            0,
            &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: "ب \u{FDFD}".into(), ..AddedText::default() }),
        );
        assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains('\u{FDFD}')), "{r:?}");
    }
}

/// One page with Helvetica (WinAnsi) and subsets of Arial: `/F2` and `/F3` are two subsets of the
/// same font, as Word writes a font split across several resources, and `/F4` repeats Helvetica.
fn text_page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 8 0 R /F5 9 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        // Subset: glyphs for a (97) and b (98) only.
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Arial /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>"
            .into(),
        // Another subset of the same Arial, with a subset tag of its own.
        "<< /Type /Font /Subtype /TrueType /BaseFont /GHIJKL+Arial /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>"
            .into(),
        // Helvetica again, in a second resource.
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        // No `/BaseFont`: `Metrics::from_dict` leaves its base-font name empty.
        "<< /Type /Font /Subtype /Type1 /Encoding /WinAnsiEncoding >>".into(),
    ];
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

#[test]
fn tracked_text_keeps_words_in_paragraphs_and_objects() {
    let expected = "TRACKED SAMPLE TEXT";
    for spacing in [1.5, 2.0001, 3.0] {
        for scale in [50.0, 100.0, 150.0] {
            for positioned in [false, true] {
                let mut show = String::new();
                if !positioned {
                    show.push('[');
                }
                for (i, c) in expected.chars().enumerate() {
                    if positioned {
                        show.push_str(&format!("({c}) Tj "));
                        if i + 1 < expected.len() {
                            show.push_str(&format!("[{}] TJ ", -spacing * 100.0));
                        }
                    } else {
                        show.push_str(&format!("({c}) "));
                        if i + 1 < expected.len() {
                            show.push_str(&format!("{} ", -spacing * 100.0));
                        }
                    }
                }
                if !positioned {
                    show.push_str("] TJ");
                }
                let doc = text_page(&format!("BT /F1 10 Tf {scale} Tz 72 700 Td {show} ET"));
                assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, expected, "{spacing}, {scale}, {positioned}");
                assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, expected);
                assert_eq!(crate::objects::editable_objects(&doc, 0).unwrap()[0].text.as_deref(), Some(expected));
            }
        }
        let doc = text_page(&format!("BT /F1 10 Tf {spacing} Tc 72 700 Td ({expected}) Tj ET"));
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, expected);
    }
}

#[test]
fn single_glyph_words_and_cells_are_not_tracking() {
    for (content, expected) in [
        ("BT /F1 10 Tf 72 700 Td [(a) -1000 (b) -1000 (c) -1000 (d)] TJ ET", "a b c d"),
        ("BT /F1 10 Tf 72 700 Td [(1) -2500 (2) -2500 (3) -2500 (4) -2500 (5)] TJ ET", "1 2 3 4 5"),
        ("BT /F1 10 Tf 72 700 Td [(A) -300 (B) -1200 (C) -1200 (D) -1200 (E) -300 (F) -1200 (G)] TJ ET", "A B C D E F G"),
    ] {
        assert_eq!(text::text_blocks(&text_page(content), 0).unwrap()[0].text, expected, "{content}");
    }
    let mut s = String::from("BT /F1 10 Tf ");
    for (i, c) in "12345".chars().enumerate() {
        s.push_str(&format!("1 0 0 1 {} 700 Tm ({c}) Tj ", 72 + i * 12));
    }
    s.push_str("ET");
    assert_eq!(text::text_blocks(&text_page(&s), 0).unwrap()[0].text, "1 2 3 4 5");
}

#[test]
fn tracked_text_preserves_larger_word_gaps() {
    for show in [
        "[(T) -300 (E) -300 (S) -300 (T) -600 (W) -300 (O) -300 (R) -300 (D)] TJ",
        "(T) Tj [-300] TJ (E) Tj [-300] TJ (S) Tj [-300] TJ (T) Tj [-600] TJ (W) Tj [-300] TJ (O) Tj [-300] TJ (R) Tj [-300] TJ (D) Tj",
        "[(TEST) -300 (WORD)] TJ",
        "(TEST) Tj [-300] TJ (W) Tj [-300] TJ (O) Tj [-300] TJ (R) Tj [-300] TJ (D) Tj",
    ] {
        let doc = text_page(&format!("BT /F1 10 Tf 72 700 Td {show} ET"));
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, "TEST WORD", "{show}");
    }
}

#[test]
fn tracked_text_positions_do_not_count_character_spacing_twice() {
    let expected = "TRACKED SAMPLE TEXT";
    for char_spacing in [0.0, 2.0] {
        for transform in ["1 0 0 1 0 0", "0 1 -1 0 800 0"] {
            let mut content = format!("q {transform} cm BT /F1 10 Tf {char_spacing} Tc ");
            let mut x = 72.0;
            for c in expected.chars() {
                content.push_str(&format!("1 0 0 1 {x:.4} 700 Tm ({c}) Tj "));
                x += pdfcraft_fonts::helvetica_width(&c.to_string(), 10.0) + 2.0001;
            }
            content.push_str("ET Q");
            let doc = text_page(&content);
            assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, expected);
            assert_eq!(crate::objects::editable_objects(&doc, 0).unwrap()[0].text.as_deref(), Some(expected));
        }
    }
}

fn descriptor_text_page() -> Document {
    let content = "BT /F1 12 Tf 72 700 Td (ab) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+NeutralFace /FirstChar 97 /LastChar 98 /Widths [500 500] /FontDescriptor 6 0 R /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /FontDescriptor /FontName /NeutralFace /Flags 262208 /ItalicAngle -12 /FontWeight 700 /Ascent 900 /Descent -250 >>".into(),
    ];
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

/// The Japanese fallback face comes from craft-fonts, an optional build input.
fn without_craft_fonts(test: &str) -> bool {
    if pdfcraft_fonts::document_japanese_font().is_some() {
        return false;
    }
    eprintln!("skipping {test}: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
    true
}

#[test]
fn japanese_line_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_line_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "25362738こんにちは、お元気ですか？ 2.3444";
    let result = text::replace_line(&mut doc, 0, 0, replacement).unwrap();
    assert_eq!(result.substituted.as_deref(), Some("BIZ UDPGothic Type3"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCJp"), "{content}");
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, replacement);
}

fn styled_text_page(base: &str) -> Document {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
    let r = pdfcraft_cos::ObjRef::new(5, 0);
    let mut font = doc.get(r).as_dict().unwrap().clone();
    font.set(b"BaseFont".to_vec(), pdfcraft_cos::Object::name(base));
    doc.set(r, pdfcraft_cos::Object::Dict(font));
    doc
}

fn fallback_paths(doc: &Document) -> Vec<Vec<u8>> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(b"PCJp").unwrap());
    let procs = doc.resolve(font.as_dict().unwrap().get(b"CharProcs").unwrap());
    procs
        .as_dict()
        .unwrap()
        .iter()
        .map(|(_, object)| match &*doc.resolve(object) {
            pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
            other => panic!("expected a glyph stream, got {other:?}"),
        })
        .collect()
}

#[test]
fn japanese_line_fallback_uses_real_source_family_and_weight() {
    if without_craft_fonts("japanese_line_fallback_uses_real_source_family_and_weight") {
        return;
    }
    let mut sans = styled_text_page("Helvetica");
    let mut bold = descriptor_text_page();
    let mut serif = styled_text_page("ABCDEF+ShipporiMincho-Regular");
    let replacement = "見本商会　御中";
    text::replace_line(&mut sans, 0, 0, replacement).unwrap();
    text::replace_line(&mut bold, 0, 0, replacement).unwrap();
    text::replace_line(&mut serif, 0, 0, replacement).unwrap();
    assert_ne!(fallback_paths(&sans), fallback_paths(&serif), "sans source must not use Mincho outlines");
    if pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        assert_ne!(fallback_paths(&sans), fallback_paths(&bold), "bold must change the actual glyph paths");
    }
    for doc in [&sans, &bold, &serif] {
        assert_eq!(text::text_lines(&reopen(doc), 0).unwrap()[0].text, replacement);
    }
}

#[test]
fn japanese_paragraph_fallback_honors_requested_family_and_weight() {
    if without_craft_fonts("japanese_paragraph_fallback_honors_requested_family_and_weight") {
        return;
    }
    let replacement = "見本商会　御中";
    let mut regular = styled_text_page("Helvetica");
    let mut bold = styled_text_page("Helvetica");
    let mut serif = styled_text_page("Helvetica");
    text::replace_block(&mut regular, 0, 0, replacement).unwrap();
    text::rewrite_block(
        &mut bold,
        0,
        0,
        Some(replacement),
        &text::BlockStyle { family: Some((added::Family::Helvetica, true, false)), ..Default::default() },
    )
    .unwrap();
    text::rewrite_block(
        &mut serif,
        0,
        0,
        Some(replacement),
        &text::BlockStyle { family: Some((added::Family::Times, false, false)), ..Default::default() },
    )
    .unwrap();
    assert_ne!(fallback_paths(&regular), fallback_paths(&serif), "requested serif must change actual outlines");
    if pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        assert_ne!(fallback_paths(&regular), fallback_paths(&bold), "requested bold must change actual outlines");
        let mut source_bold = styled_text_page("Helvetica-Bold");
        text::replace_block(&mut source_bold, 0, 0, replacement).unwrap();
        assert_eq!(fallback_paths(&bold), fallback_paths(&source_bold));
    }
    for doc in [&regular, &bold, &serif] {
        assert_eq!(text::text_blocks(&reopen(doc), 0).unwrap()[0].text, replacement);
    }
}

#[test]
fn japanese_fallback_reports_the_face_after_save_and_reopen() {
    if without_craft_fonts("japanese_fallback_reports_the_face_after_save_and_reopen") {
        return;
    }
    // F2 is (a subset of) Arial: a regular sans face, so the fallback is the regular Gothic.
    let face = pdfcraft_fonts::document_japanese_font_for_style(false, false).unwrap();
    let expected = format!("{}-{}", face.family, face.style).replace(' ', "");
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
        let replacement = "見本商会　御中";
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        for d in [&doc, &reopen(&doc)] {
            let lines = text::text_lines(d, 0).unwrap();
            assert_eq!(lines[0].text, replacement);
            assert_eq!(lines[0].base_font, expected, "report the real fallback face, not an empty BaseFont");
            assert_eq!(text::text_blocks(d, 0).unwrap()[0].base_font, expected);
        }
    }
}

#[test]
fn cyrillic_in_a_serif_line_falls_back_to_a_face_that_has_it() {
    if without_craft_fonts("cyrillic_in_a_serif_line_falls_back_to_a_face_that_has_it") {
        return;
    }
    let replacement = "Привет, world";
    let has_all = |f: &pdfcraft_fonts::CraftFont| replacement.chars().all(|ch| pdfcraft_fonts::japanese_glyph_from(f, ch).is_ok());
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.covers("Jpan") && has_all(f)) {
        eprintln!("skipping: no Japanese fallback face has Cyrillic");
        return;
    }
    for paragraph in [false, true] {
        let mut doc = styled_text_page("Times-Roman");
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, replacement);
    }
}

/// #2593: the craft-fonts faces are Japanese, and part of the Cyrillic (Uzbek қ ғ ҳ, Kazakh ә ө ү
/// ұ һ) is in none of them. The bundled Inter face has it, so the replacement is written instead of
/// being refused, and a build without craft-fonts can do it too.
#[test]
fn uzbek_and_kazakh_cyrillic_use_the_bundled_latin_face() {
    for replacement in ["The quick brown fox jumps over the lazy dog қизил", "әже", "ўзбекча ғўза"] {
        for paragraph in [false, true] {
            let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
            let edit =
                if paragraph { text::replace_block(&mut doc, 0, 0, replacement) } else { text::replace_line(&mut doc, 0, 0, replacement) }.unwrap();
            assert_eq!(edit.substituted.as_deref(), Some("Inter Type3"), "{replacement}: {edit:?}");
            let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
            assert!(content.contains("/PCLat"), "{replacement}: {content}");
            let lines = text::text_lines(&reopen(&doc), 0).unwrap();
            assert_eq!(lines[0].text, replacement);
            assert_eq!(lines[0].base_font, "Inter-Regular", "the generated font reports the face it drew");
        }
    }
}

/// The Japanese faces keep first refusal for the Cyrillic they do carry, so Russian text keeps the
/// face it was drawn with before the bundled Latin face was added.
#[test]
fn cyrillic_the_japanese_faces_carry_keeps_them() {
    if without_craft_fonts("cyrillic_the_japanese_faces_carry_keeps_them") {
        return;
    }
    let face = pdfcraft_fonts::document_japanese_font_for_style(false, false).unwrap();
    if pdfcraft_fonts::japanese_glyph_from(face, 'ф').is_err() {
        eprintln!("skipping: the Japanese fallback face has no Cyrillic");
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let edit = text::replace_line(&mut doc, 0, 0, "Привет, world").unwrap();
    let expected = format!("{} Type3", face.family);
    assert_eq!(edit.substituted.as_deref(), Some(expected.as_str()));
}

#[test]
fn japanese_paragraph_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_paragraph_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "こんにちは、お元気ですか？ 2.3444 日本語の文章";
    text::replace_block(&mut doc, 0, 0, replacement).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].text, replacement);
}

/// The first `n` Japanese characters, in code point order, that every craft-fonts face can draw, so
/// whichever face an edit picks has them all. Without craft-fonts there are no faces, every character
/// passes, and the text is still long.
fn covered_japanese_chars(n: usize) -> String {
    let faces = pdfcraft_fonts::document_japanese_fonts_for_style(false, false);
    (0x4E00..0xA000)
        .filter_map(char::from_u32)
        .filter(|ch| faces.iter().all(|face| pdfcraft_fonts::japanese_glyph_from(face, *ch).is_ok()))
        .take(n)
        .collect()
}

#[test]
fn fallback_fonts_nothing_shows_any_more_leave_with_a_rewrite() {
    // `PCJp1` is selected by the page, `PCJp2` only by a form that uses the page's resources, `PCJp`
    // by nothing; `PCJpOther` is a Type 3 font of the file's own that only shares the prefix.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Some text) Tj ET BT /PCJp1 12 Tf ET q /Fm0 Do Q");
    let generated = |doc: &mut Document, name: &str| {
        doc.add(Object::Dict(Dict::from_iter([
            (b"Type".to_vec(), Object::name("Font")),
            (b"Subtype".to_vec(), Object::name("Type3")),
            (b"Name".to_vec(), Object::name(name)),
        ])))
    };
    let (unused, page_shown, form_shown, own) =
        (generated(&mut doc, "PCJapanese"), generated(&mut doc, "PCJapanese"), generated(&mut doc, "PCJapanese"), generated(&mut doc, "Other"));
    let form = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([(b"Subtype".to_vec(), Object::name("Form")), (b"BBox".to_vec(), Object::Array(vec![Object::Int(0); 4]))]),
        b"BT /PCJp2 12 Tf ET".to_vec(),
    )));
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let mut fonts = Dict::from_iter([(b"F1".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(5, 0)))]);
    for (name, font) in [("PCJp", unused), ("PCJp1", page_shown), ("PCJp2", form_shown), ("PCJpOther", own)] {
        fonts.set(name.as_bytes().to_vec(), Object::Ref(font));
    }
    let resources = Dict::from_iter([
        (b"Font".to_vec(), Object::Dict(fonts)),
        (b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Fm0".to_vec(), Object::Ref(form))]))),
    ]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
    let names = |doc: &Document| {
        let page = pdfcraft_model::pages(doc).remove(0);
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
        let mut names: Vec<String> =
            doc.resolve(res.get(b"Font").unwrap()).as_dict().unwrap().iter().map(|(n, _)| String::from_utf8_lossy(n).into_owned()).collect();
        names.sort();
        names
    };
    // A rewrite that doesn't reach the fallback fonts still drops the one nothing shows.
    text::rewrite_block(&mut doc, 0, 0, Some("Other text"), &text::BlockStyle::default()).unwrap();
    assert_eq!(names(&reopen(&doc)), ["F1", "PCJp1", "PCJp2", "PCJpOther"]);

    // A form using the page's resources whose content can't be read might show any of them: they
    // all stay.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Some text) Tj ET q /Fm0 Do Q");
    let unused = generated(&mut doc, "PCJapanese");
    let broken = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![Object::Int(0); 4])),
            (b"Filter".to_vec(), Object::name("FlateDecode")),
        ]),
        b"not deflate data".to_vec(),
    )));
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let fonts = Dict::from_iter([(b"F1".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(5, 0))), (b"PCJp".to_vec(), Object::Ref(unused))]);
    let resources = Dict::from_iter([
        (b"Font".to_vec(), Object::Dict(fonts)),
        (b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Fm0".to_vec(), Object::Ref(broken))]))),
    ]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    text::rewrite_block(&mut doc, 0, 0, Some("Other text"), &text::BlockStyle::default()).unwrap();
    assert_eq!(names(&doc), ["F1", "PCJp"]);
}

#[test]
fn rewriting_japanese_twice_keeps_only_the_fallback_fonts_it_shows() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    if text::rewrite_block(&mut doc, 0, 0, Some("日本語の文章"), &text::BlockStyle::default()).is_err() {
        assert!(pdfcraft_fonts::document_japanese_font().is_none());
        eprintln!("built without a craft-fonts Japanese face: the Type 3 path is checked with CRAFT_FONTS_DIR");
        return;
    }
    let first: Vec<_> = fallback_fonts(&doc).into_iter().map(|(_, f)| f).collect();
    text::rewrite_block(&mut doc, 0, 0, Some("別の文章です"), &text::BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    let after = fallback_fonts(&doc);
    assert!(!after.is_empty());
    assert!(after.iter().all(|(_, f)| !first.contains(f)), "the first rewrite's fonts are gone");
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(after.iter().all(|(name, _)| content.contains(&format!("/{} ", String::from_utf8_lossy(name)))), "{content}");
}

/// The page's Type 3 fallback fonts (`PCJp`, `PCJp1`, ...) by resource name.
fn fallback_fonts(doc: &Document) -> Vec<(Vec<u8>, Dict)> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    fonts
        .as_dict()
        .unwrap()
        .iter()
        .filter(|(name, _)| name.starts_with(b"PCJp"))
        .map(|(name, font)| (name.clone(), doc.resolve(font).as_dict().unwrap().clone()))
        .collect()
}

/// More unique fallback glyphs than one Type 3 font holds (240) are split across several generated
/// fonts, each within that limit, and the text still reads back as one line.
#[test]
fn japanese_replacement_over_240_unique_glyphs_is_split_across_fallback_fonts() {
    let replacement = covered_japanese_chars(300);
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    if without_craft_fonts("japanese_replacement_over_240_unique_glyphs_is_split_across_fallback_fonts") {
        // Without craft-fonts the long text gets the clear error, not the old limit message.
        let err = text::replace_line(&mut doc, 0, 0, &replacement).unwrap_err().to_string();
        assert!(err.contains("CRAFT_FONTS_DIR"), "{err}");
        return;
    }
    text::replace_line(&mut doc, 0, 0, &replacement).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.len(), 1, "one line: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(lines[0].text, replacement);
    let fonts = fallback_fonts(&reopened);
    assert_eq!(fonts.len(), 2, "300 glyphs take a 240-glyph font and a 60-glyph font");
    let mut glyphs = 0;
    for (name, font) in &fonts {
        let procs = reopened.resolve(font.get(b"CharProcs").unwrap());
        let count = procs.as_dict().unwrap().len();
        let last = font.get(b"LastChar").and_then(|o| o.as_int()).unwrap_or(i64::MAX);
        assert!(count <= 240 && last <= 240, "{name:?}: {count} glyphs, LastChar {last}");
        glyphs += count;
    }
    assert_eq!(glyphs, 300);
}

#[test]
fn styled_paragraph_batches_cyrillic_without_changing_source_fonts() {
    if without_craft_fonts("styled_paragraph_batches_cyrillic_without_changing_source_fonts") {
        return;
    }
    let mut doc = text_page(
        "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (The quick brown fox jumps over the lazy dog) Tj T* (and keeps running through the long field.) Tj ET",
    );
    let replacement = "The quick brown fox jumps over the lazy dog шалом\nand keeps running through the long field.";
    text::rewrite_block(&mut doc, 0, 0, Some(replacement), &text::BlockStyle { width: Some(520.0), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    assert_eq!(fallback_fonts(&doc).len(), 1, "the five letters must share one font");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.iter().any(|l| l.base_font == "Helvetica" && l.text.starts_with("The quick brown fox")), "{lines:?}");
    let fallback: Vec<_> = lines.iter().filter(|l| l.font.starts_with("PCJp")).collect();
    assert_eq!(fallback.len(), 1, "one word must be one text run: {fallback:?}");
    assert_eq!(fallback[0].text, "шалом");
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>().replace(' ', ""), replacement.replace([' ', '\n'], ""));
}

#[test]
fn styled_paragraph_over_240_fallback_glyphs_splits_and_refuses_atomically() {
    let replacement = format!("{}\nLatin", covered_japanese_chars(300));
    let mut doc = text_page("BT /F1 12 Tf 14.4 TL 72 700 Td (Original) Tj T* (Latin) Tj ET");
    let original = write_incremental(&doc, &SaveOptions::default()).unwrap();
    if without_craft_fonts("styled_paragraph_over_240_fallback_glyphs_splits_and_refuses_atomically") {
        assert!(text::replace_block(&mut doc, 0, 0, &replacement).is_err());
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), original);
        return;
    }
    text::rewrite_block(&mut doc, 0, 0, Some(&replacement), &text::BlockStyle { width: Some(14000.0), ..Default::default() }).unwrap();
    let reopened = reopen(&doc);
    let fonts = fallback_fonts(&reopened);
    assert_eq!(fonts.len(), 2, "300 glyphs need two fonts, not an error at glyph 241");
    let counts: Vec<_> = fonts.iter().map(|(_, f)| reopened.dict(f.get(b"CharProcs").unwrap()).unwrap().len()).collect();
    assert_eq!(counts.iter().sum::<usize>(), 300);
    assert!(counts.iter().all(|n| *n <= 240));
    assert_eq!(text::text_lines(&reopened, 0).unwrap().iter().map(|l| l.text.as_str()).collect::<String>(), replacement.replace('\n', ""));
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let snapshot = doc.clone();
    let err = text::replace_block(&mut doc, 0, 0, &format!("{replacement}\u{10ffff}")).unwrap_err();
    assert!(err.to_string().contains("U+10FFFF"), "{err}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    assert_eq!(write_incremental(&snapshot, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn styled_fallback_batches_keep_face_choices_and_range_styles() {
    if without_craft_fonts("styled_fallback_batches_keep_face_choices_and_range_styles") {
        return;
    }
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        eprintln!("skipping: no bold Gothic face");
        return;
    }
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
    let range = |range, family, bold| text::RangeStyle {
        range,
        family: Some((family, bold, false)),
        resource: None,
        size: None,
        color: None,
        underline: None,
        char_spacing: None,
        scale: None,
    };
    let style = text::BlockStyle {
        width: Some(520.0),
        ranges: vec![
            range(3..6, added::Family::Times, false),
            range(6..9, added::Family::Helvetica, true),
            range(9..11, added::Family::Times, false),
        ],
        ..Default::default()
    };
    let original = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let err = text::rewrite_block(&mut doc, 0, 0, Some("見見見ш見ш\nLatin\u{10ffff}"), &style).unwrap_err();
    assert!(err.to_string().contains("U+10FFFF"), "{err}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), original, "all faces must validate before writing fallback objects");
    text::rewrite_block(&mut doc, 0, 0, Some("見見見ш見ш\nLatin"), &style).unwrap();
    let doc = reopen(&doc);
    let fonts = fallback_fonts(&doc);
    let mut summary: Vec<_> = fonts
        .iter()
        .map(|(_, f)| {
            let face = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, f).base_font;
            let count = doc.dict(f.get(b"CharProcs").unwrap()).unwrap().len();
            (face, count)
        })
        .collect();
    summary.sort();
    assert_eq!(
        summary,
        [
            ("BIZUDMincho-Regular".into(), 1),
            ("BIZUDPGothic-Bold".into(), 1),
            ("BIZUDPGothic-Regular".into(), 2),
            ("ShipporiMincho-Regular".into(), 1)
        ]
    );
    assert_eq!(text::text_lines(&doc, 0).unwrap().iter().map(|l| l.text.as_str()).collect::<String>(), "見見見ш見шLatin");
}

#[test]
fn styled_fallback_budget_is_enforced_before_objects_are_written() {
    let glyphs = covered_japanese_chars(65);
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
    let original = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let snapshot = doc.clone();
    let style = text::BlockStyle { width: Some(14000.0), ..Default::default() };
    let over_budget = format!("{glyphs}\nLatin");
    let err = text::rewrite_block_with_limit(&mut doc, 0, 0, Some(&over_budget), &style, 1).unwrap_err().to_string();
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), original, "refusal must add no objects");
    assert_eq!(write_incremental(&snapshot, &SaveOptions::default()).unwrap(), original);
    if without_craft_fonts("styled_fallback_budget_is_enforced_before_objects_are_written") {
        assert!(err.contains("CRAFT_FONTS_DIR"), "{err}");
        return;
    }
    assert!(err.contains("more than 64 fallback glyphs"), "{err}");
    // One glyph per font gives a small, real budget of 64 distinct glyphs. At the
    // boundary a repeated glyph is free: this is not a limit on text length.
    let at_budget: String = glyphs.chars().take(64).chain(glyphs.chars().take(1)).collect();
    text::rewrite_block_with_limit(&mut doc, 0, 0, Some(&format!("{at_budget}\nLatin")), &style, 1).unwrap();
    let doc = reopen(&doc);
    assert_eq!(fallback_fonts(&doc).len(), 64);
    assert_eq!(text::text_lines(&doc, 0).unwrap().iter().map(|l| l.text.as_str()).collect::<String>(), format!("{at_budget}Latin"));
}

#[test]
fn styled_fallback_same_base_font_and_character_keep_both_weights() {
    if without_craft_fonts("styled_fallback_same_base_font_and_character_keep_both_weights") {
        return;
    }
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        eprintln!("skipping: no bold Gothic face");
        return;
    }
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (alpha ) Tj /F2 12 Tf (beta ) Tj ET");
    let source = doc.get(pdfcraft_cos::ObjRef::new(5, 0)).as_dict().unwrap().clone();
    for (object, weight) in [(5, 400), (6, 700)] {
        let mut font = source.clone();
        // The same BaseFont can have different descriptor weights. Changing a
        // RangeStyle family would also rename BaseFont and miss this cache key.
        font.set(b"BaseFont".to_vec(), Object::name("NeutralFace"));
        let mut descriptor = Dict::new();
        descriptor.set(b"FontName".to_vec(), Object::name("NeutralFace"));
        descriptor.set(b"FontWeight".to_vec(), Object::Int(weight));
        font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
        doc.set(pdfcraft_cos::ObjRef::new(object, 0), Object::Dict(font));
    }
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].base_font, lines[1].base_font);
    assert_eq!((lines[0].bold, lines[1].bold), (false, true));
    // Unchanged words anchor each inserted copy of the same letter in its own source style.
    let block = text::text_blocks(&doc, 0).unwrap().remove(0);
    let mut inherited = Vec::new();
    for (range, source) in block.edited_spans("alpha ш beta ш") {
        for _ in "alpha ш beta ш".get(range).unwrap().chars().filter(|ch| *ch == 'ш') {
            inherited.push(lines[source].bold);
        }
    }
    assert_eq!(inherited, [false, true], "the fixture must exercise both weights of the same BaseFont");
    text::replace_block(&mut doc, 0, 0, "alpha ш beta ш").unwrap();
    let doc = reopen(&doc);
    let fonts = fallback_fonts(&doc);
    let mut names: Vec<_> = fonts.iter().map(|(_, f)| pdfcraft_fonts::pdf::Metrics::from_dict(&doc, f).base_font).collect();
    names.sort();
    assert_eq!(names, ["BIZUDPGothic-Bold", "BIZUDPGothic-Regular"]);
    let outlines: Vec<_> = fonts
        .iter()
        .map(|(_, f)| {
            let procs = doc.dict(f.get(b"CharProcs").unwrap()).unwrap();
            assert_eq!(procs.len(), 1, "each weight needs just the shared letter");
            stream_bytes(&doc, procs.iter().next().unwrap().1).unwrap()
        })
        .collect();
    assert_ne!(outlines[0], outlines[1], "weight must change the outline, not only the descriptor");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), "alpha ш beta ш");
}

#[test]
fn styled_fallback_shards_keep_text_advances_and_alignment() {
    if without_craft_fonts("styled_fallback_shards_keep_text_advances_and_alignment") {
        return;
    }
    let source = "BT /F1 12 Tf 0.5 Tc 5 Tw 80 Tz 14.4 TL 72 700 Td (Original row) Tj T* (Second row) Tj ET";
    let style =
        text::BlockStyle { width: Some(520.0), align: Some(added::Align::Center), char_spacing: Some(0.7), scale: Some(80.0), ..Default::default() };
    let mut whole = text_page(source);
    let mut split = text_page(source);
    let replacement = "Latin шалом ш\nSecond row";
    text::rewrite_block_with_limit(&mut whole, 0, 0, Some(replacement), &style, 240).unwrap();
    text::rewrite_block_with_limit(&mut split, 0, 0, Some(replacement), &style, 2).unwrap();
    let (whole, split) = (reopen(&whole), reopen(&split));
    assert_eq!(fallback_fonts(&whole).len(), 1);
    assert_eq!(fallback_fonts(&split).len(), 3);
    let (one, many) = (text::text_lines(&whole, 0).unwrap(), text::text_lines(&split, 0).unwrap());
    assert_eq!(one.len(), many.len(), "{one:?} vs {many:?}");
    for (a, b) in one.iter().zip(&many) {
        assert_eq!(a.text, b.text);
        for (a, b) in a.rect.iter().zip(b.rect) {
            assert!((a - b).abs() < 0.002, "sharding must preserve alignment and advances: {a} vs {b}");
        }
    }
}

/// A replacement whose last glyph no face draws (in a later font) changes nothing: neither the
/// document nor a snapshot taken before the edit, which is what undo restores.
#[test]
fn japanese_replacement_missing_a_glyph_after_240_changes_nothing() {
    if without_craft_fonts("japanese_replacement_missing_a_glyph_after_240_changes_nothing") {
        return;
    }
    // The first CJK character that no face draws: the 242nd unique character, in a second font.
    let faces = pdfcraft_fonts::document_japanese_fonts_for_style(false, false);
    let missing = (0x4E00u32..0x9FFF)
        .filter_map(char::from_u32)
        .find(|ch| faces.iter().all(|face| pdfcraft_fonts::japanese_glyph_from(face, *ch).is_err()))
        .expect("some CJK character is outside every face");
    let replacement = format!("{}{missing}", covered_japanese_chars(241));
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let snapshot = doc.clone();
    let err = text::replace_line(&mut doc, 0, 0, &replacement).unwrap_err().to_string();
    assert!(err.contains(&format!("U+{:04X}", missing as u32)), "{err}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved);
    assert_eq!(write_incremental(&snapshot, &SaveOptions::default()).unwrap(), saved);
}

/// Runs that change font move nothing: with two glyphs per font the text reads back the same, in the
/// same box as with one font, and the font is switched only where the text moves to another font.
#[test]
fn japanese_fallback_split_across_fonts_keeps_order_and_positions() {
    if without_craft_fonts("japanese_fallback_split_across_fonts_keeps_order_and_positions") {
        return;
    }
    // 見 and 商 are in the first font, 御 in the second: runs [見商], [御], [見], three switches.
    let replacement = "見商御見";
    let mut whole = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let mut split = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut whole, 0, 0, replacement, 240).unwrap();
    text::replace_line_with_limit(&mut split, 0, 0, replacement, 2).unwrap();
    let (whole, split) = (reopen(&whole), reopen(&split));
    assert_eq!(fallback_fonts(&whole).len(), 1);
    assert_eq!(fallback_fonts(&split).len(), 2);
    let (one, many) = (text::text_lines(&whole, 0).unwrap(), text::text_lines(&split, 0).unwrap());
    assert_eq!(many.len(), 1, "one line: {:?}", many.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(many[0].text, replacement);
    for (a, b) in one[0].rect.iter().zip(many[0].rect.iter()) {
        assert!((a - b).abs() < 1e-6, "box {:?} vs {:?}", one[0].rect, many[0].rect);
    }
    // One `Tf` per run plus the page's own switch to F2 and the switch back after the text.
    let content = String::from_utf8_lossy(&page_content_bytes(&split, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 5, "{content}");
    let content = String::from_utf8_lossy(&page_content_bytes(&whole, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 3, "{content}");
}

/// Only a change of font gets a `Tf`: alternating characters with one glyph per font switch at every
/// character, and a run of one font's characters switches nowhere.
#[test]
fn japanese_fallback_switches_font_only_between_runs() {
    if without_craft_fonts("japanese_fallback_switches_font_only_between_runs") {
        return;
    }
    let alternating = "見商".repeat(500);
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut doc, 0, 0, &alternating, 1).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, alternating);
    let content = String::from_utf8_lossy(&page_content_bytes(&reopened, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 1000 + 2, "1000 runs, the F2 switch before and after");
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut doc, 0, 0, "見見見", 1).unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&reopen(&doc), 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 1 + 1 + 1, "{content}");
}

/// A paragraph set in several fallback fonts reads back as one block with the same text, and it
/// wraps across lines.
#[test]
fn japanese_paragraph_split_across_fonts_reads_back_as_one_block() {
    if without_craft_fonts("japanese_paragraph_split_across_fonts_reads_back_as_one_block") {
        return;
    }
    // Spaces are glyphs too, so two glyphs per font needs several fonts; the narrow box wraps the text.
    let replacement = "見 商 御 見 商";
    let style = text::BlockStyle { width: Some(40.0), ..Default::default() };
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::rewrite_block_with_limit(&mut doc, 0, 0, Some(replacement), &style, 2).unwrap();
    let reopened = reopen(&doc);
    assert!(fallback_fonts(&reopened).len() >= 2);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.len(), 1, "one paragraph: {:?}", blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
    assert_eq!(blocks[0].text, replacement);
    assert!(blocks[0].lines.len() > 1, "wrapped to {} line(s)", blocks[0].lines.len());
}

/// A paragraph whose move or width is refused (not finite, or drawn in a space it cannot be moved in)
/// leaves the document as it was: the fallback fonts are written only once the edit is accepted.
#[test]
fn japanese_paragraph_refused_before_its_fallback_fonts_are_written() {
    if without_craft_fonts("japanese_paragraph_refused_before_its_fallback_fonts_are_written") {
        return;
    }
    let plain = "BT /F2 12 Tf 72 700 Td (ab) Tj ET";
    let cases = [
        (plain, text::BlockStyle { offset: Some([f64::NAN, 0.0]), ..Default::default() }, "can't be moved that far"),
        (plain, text::BlockStyle { width: Some(f64::INFINITY), ..Default::default() }, "width must be a number"),
        (
            "0 0 0 0 0 0 cm BT /F2 12 Tf 72 700 Td (ab) Tj ET",
            text::BlockStyle { offset: Some([5.0, 0.0]), ..Default::default() },
            "can't be moved in",
        ),
    ];
    for (content, style, message) in cases {
        let mut doc = text_page(content);
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let err = text::rewrite_block(&mut doc, 0, 0, Some("見本商会　御中"), &style).unwrap_err().to_string();
        assert!(err.contains(message), "{err}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved, "{message}: the document changed");
    }
}

/// Every Type 3 fallback glyph starts with a well-formed `d1`: six operands, the second 0, and a
/// box that encloses every point of the glyph. Acrobat shows a bullet for a glyph whose `d1`
/// has the wrong operand count.
#[test]
fn type3_fallback_glyphs_declare_a_well_formed_d1() {
    if without_craft_fonts("type3_fallback_glyphs_declare_a_well_formed_d1") {
        return;
    }
    // Cyrillic is respaced (shifted by its side bearing), and its box must move with it.
    let mut glyphs = Vec::new();
    for replacement in ["見本商会 御中", "София 2027"] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
        text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        glyphs.extend(fallback_paths(&reopen(&doc)));
    }
    assert!(!glyphs.is_empty());
    for glyph in glyphs {
        let text = String::from_utf8(glyph).unwrap();
        let mut lines = text.lines();
        let d1: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
        assert_eq!(d1.len(), 7, "six operands and d1: {d1:?}");
        assert_eq!(d1[6], "d1");
        let n: Vec<f64> = d1[..6].iter().map(|v| v.parse().unwrap()).collect();
        assert_eq!(n[1], 0.0, "wy");
        assert!(n[2] <= n[4] && n[3] <= n[5], "box: {n:?}");
        for line in lines.filter(|l| l.ends_with(" m") || l.ends_with(" l")) {
            let p: Vec<f64> = line.split_whitespace().take(2).map(|v| v.parse().unwrap()).collect();
            assert!(p[0] >= n[2] && p[0] <= n[4] && p[1] >= n[3] && p[1] <= n[5], "{line} outside {n:?}");
        }
    }
}

/// Shippori Mincho, the serif fallback, has no Cyrillic: Bulgarian replacement text in a serif
/// line takes a fallback face that has every letter instead of failing on the first one.
#[test]
fn cyrillic_replacement_in_a_serif_line_uses_a_face_with_the_letters() {
    if without_craft_fonts("cyrillic_replacement_in_a_serif_line_uses_a_face_with_the_letters") {
        return;
    }
    let replacement = "София 2027";
    for paragraph in [false, true] {
        let mut doc = styled_text_page("ABCDEF+Garamond");
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines[0].text, replacement);
        // Letters are spaced by their shape, not one em each (the faces' full-width Cyrillic).
        let widths = fallback_widths(&doc);
        assert!(widths.iter().all(|w| *w > 300.0 && *w < 900.0), "{widths:?}");
        assert!(widths.windows(2).any(|w| w[0] != w[1]), "proportional: {widths:?}");
    }
}

/// The `/Widths` of the page's Type 3 fallback font, for its Cyrillic letters.
fn fallback_widths(doc: &Document) -> Vec<f64> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(b"PCJp").unwrap());
    let font = font.as_dict().unwrap();
    let widths = doc.resolve(font.get(b"Widths").unwrap()).as_array().unwrap().iter().filter_map(|w| w.as_f64()).collect::<Vec<_>>();
    // "София 2027": the first five codes are the letters.
    widths[..5].to_vec()
}

#[test]
fn japanese_paragraph_keeps_ideographic_spaces() {
    if without_craft_fonts("japanese_paragraph_keeps_ideographic_spaces") {
        return;
    }
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original paragraph) Tj ET");
    let replacement = "　見本商会　　御中　";
    let style = text::BlockStyle { width: Some(400.0), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some(replacement), &style).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, replacement);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].text, replacement);
    // Restyling the saved paragraph without new text must also preserve its spacing.
    let mut doc = reopened;
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { size: Some(14.0), ..style }).unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, replacement);
}

/// Without craft-fonts there is no Japanese face: editing in Japanese is a clear error that
/// leaves the page untouched, never a panic; Latin edits work as before.
#[test]
fn japanese_edit_without_craft_fonts_is_a_clear_error() {
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let line = text::replace_line(&mut doc, 0, 0, "日本語の文字");
    let block = text::replace_block(&mut doc, 0, 0, "日本語の文字");
    if pdfcraft_fonts::document_japanese_font().is_some() {
        eprintln!("built with craft-fonts: the Japanese edits succeed (checked by the tests above)");
        assert!(line.is_ok() && block.is_ok());
        return;
    }
    for result in [line.map(|_| ()), block.map(|_| ())] {
        let Err(EditError::Invalid(msg)) = result else { panic!("expected a clear error, got {result:?}") };
        assert!(msg.contains("Japanese fallback font") && msg.contains("CRAFT_FONTS_DIR"), "{msg}");
    }
    assert_eq!(page_content_bytes(&doc, 0), before);
    let latin = text::replace_line(&mut doc, 0, 0, "Hello").unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "Hello", "{latin:?}");
}
#[test]
fn font_descriptor_style_is_exposed_even_with_a_neutral_name() {
    let doc = descriptor_text_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].bold && lines[0].italic, "{:?}", lines[0]);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert!(blocks[0].bold && blocks[0].italic, "{:?}", blocks[0]);
}

#[test]
fn mixed_font_runs_are_not_merged_into_one_source_style() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Regular) Tj /F2 12 Tf 150 700 Td (Subset) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert_ne!(lines[0].font, lines[1].font);
}

#[test]
fn missing_glyphs_preserve_source_style_in_fallback() {
    let mut doc = descriptor_text_page();
    let result = text::replace_line(&mut doc, 0, 0, "Styled €").unwrap();
    assert_eq!(result.substituted.as_deref(), Some("Helvetica-BoldOblique"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCEdHelveticaBoldOblique 12 Tf"), "{content}");
}

#[test]
fn a_subset_without_a_space_glyph_keeps_its_font_when_editing_words() {
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj /F1 12 Tf ( ) Tj /F2 12 Tf (ba) Tj ET");
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, "ab ba");
        let edit = if paragraph { text::replace_block(&mut doc, 0, 0, "ba ab ba") } else { text::replace_line(&mut doc, 0, 0, "ba ab ba") };
        assert_eq!(edit.unwrap().substituted, None);
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines[0].text, "ba ab ba");
        assert_eq!(lines[0].font, "F2");
        assert!((lines[0].rect[2] - lines[0].rect[0]) > 3.0 * 12.0);
    }
}

/// A Chrome (Skia) style subset: Type0/Identity-H over a TrueType program, CID = glyph id.
/// `/W` lists most used glyphs, writes 0 for some it dropped, and leaves `a` and `y` to `/DW`;
/// `z` has a ToUnicode entry but no glyph in the program (its CID is past the last glyph).
fn skia_subset_page() -> Document {
    // Inter's glyph ids: P 375, o 790, l 736, a 507, n 773, d 597, G 141, e 614, r 852, m 766, y 998, space 1777.
    let mut doc = text_page(
        "BT /F3 12 Tf 72 700 Td <0177031602E001FB03050255> Tj ET \
         BT /F3 12 Tf 72 650 Td <008D0266035402FE01FB030503E606F101FB0305025506F102FE031603540266> Tj ET",
    );
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange 13 beginbfchar <0177> <0050> <0316> <006F> <02E0> <006C> \
          <01FB> <0061> <0305> <006E> <0255> <0064> <008D> <0047> <0266> <0065> <0354> <0072> <02FE> <006D> <03E6> <0079> \
          <06F1> <0020> <1000> <007A> endbfchar",
    )));
    let widths: Vec<Object> = [(0x8D, 746), (0x177, 639), (0x255, 612), (0x266, 583), (0x2E0, 242), (0x2FE, 876), (0x305, 591), (0x316, 600)]
        .into_iter()
        .chain([(0x354, 376), (0x355, 0), (0x356, 0), (0x6F1, 281)])
        .flat_map(|(cid, width)| [Object::Int(cid), Object::Array(vec![Object::Int(width)])])
        .collect();
    let descriptor = Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Flags".to_vec(), Object::Int(32)),
        (b"FontFile2".to_vec(), Object::Ref(program)),
    ]);
    let cid_font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("CIDFontType2")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"CIDToGIDMap".to_vec(), Object::name("Identity")),
        (b"DW".to_vec(), Object::Int(562)),
        (b"W".to_vec(), Object::Array(widths)),
        (b"FontDescriptor".to_vec(), Object::Dict(descriptor)),
    ]);
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid_font)])),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
    ]);
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let fonts = Dict::from_iter([(b"F3".to_vec(), Object::Dict(font))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(fonts))])))).unwrap();
    doc
}

/// #2616: a subset whose ToUnicode has no space (as LaTeX and Sphinx output often has, spacing
/// words with TJ kerning) makes finding the space width read every font on the page. That ran
/// for every character of a multi-row draft, so each keystroke in a long paragraph took half a
/// second. It runs once per font now, however long the paragraph.
#[test]
fn a_long_draft_in_a_font_without_a_space_works_out_the_space_width_once() {
    // Two rows of one paragraph (14 pt apart) in the Chrome-style subset of `skia_subset_page`,
    // whose ToUnicode here has no entry for the space glyph.
    let mut doc =
        text_page("BT /F3 12 Tf 72 700 Td <0177031602E001FB03050255> Tj ET          BT /F3 12 Tf 72 686 Td <008D0266035402FE01FB030503E6> Tj ET");
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange 12 beginbfchar <0177> <0050> <0316> <006F> <02E0> <006C>           <01FB> <0061> <0305> <006E> <0255> <0064> <008D> <0047> <0266> <0065> <0354> <0072> <02FE> <006D> <03E6> <0079>           <1000> <007A> endbfchar",
    )));
    let widths: Vec<Object> =
        [(0x8D, 746), (0x177, 639), (0x1FB, 551), (0x255, 612), (0x266, 583), (0x2E0, 242), (0x2FE, 876), (0x305, 591), (0x316, 600)]
            .into_iter()
            .chain([(0x354, 376), (0x3E6, 515)])
            .flat_map(|(cid, width)| [Object::Int(cid), Object::Array(vec![Object::Int(width)])])
            .collect();
    let descriptor = Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Flags".to_vec(), Object::Int(32)),
        (b"FontFile2".to_vec(), Object::Ref(program)),
    ]);
    let cid_font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("CIDFontType2")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"CIDToGIDMap".to_vec(), Object::name("Identity")),
        (b"DW".to_vec(), Object::Int(562)),
        (b"W".to_vec(), Object::Array(widths)),
        (b"FontDescriptor".to_vec(), Object::Dict(descriptor)),
    ]);
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid_font)])),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
    ]);
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let fonts = Dict::from_iter([(b"F3".to_vec(), Object::Dict(font))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(fonts))])))).unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1, "one paragraph: {:?}", blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
    let draft = "Poland and Germany
"
    .repeat(20);
    text::SOURCE_SPACE_CALLS.with(|c| c.set(0));
    let layout = text::text_edit_layout(&doc, 0, 0, draft.trim_end(), &text::BlockStyle::default()).unwrap();
    let calls = text::SOURCE_SPACE_CALLS.with(std::cell::Cell::get);
    assert!(layout.draft_rect.is_some_and(|r| r[2] > r[0]), "laid out: {:?}", layout.draft_rect);
    assert!(calls <= 8, "the space width was worked out {calls} times for {} characters", draft.len());
}

#[test]
fn a_subset_keeps_its_font_for_glyphs_left_to_the_default_width() {
    let doc = skia_subset_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Poland", "Germany and more"]);
    // Deleting words only, and replacing a word with one drawn elsewhere on the page (`y` and `a`
    // have no /W entry): every glyph is in the subset, so the font stays.
    for (block, text) in [(1, "Germany"), (0, "Germany")] {
        let mut doc = doc.clone();
        assert_eq!(text::replace_block(&mut doc, 0, block, text).unwrap().substituted, None, "{text}");
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines.iter().find(|l| l.text == text).unwrap().font, "F3");
    }
    // `z` maps to a CID the program has no glyph for: that still needs another font.
    let mut doc = doc.clone();
    assert!(text::replace_block(&mut doc, 0, 0, "Graz").unwrap().substituted.is_some());
}

#[test]
fn spaces_in_another_subset_do_not_split_a_line() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj /F2 12 Tf ( ) Tj /F1 12 Tf (world) Tj ET");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "Hello world");
    text::replace_block(&mut doc, 0, 0, "Goodbye").unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "Goodbye");
}

#[test]
fn editor_background_removes_glyphs_but_preserves_following_text_advance() {
    for show in ["(Hello) Tj", "[(He) -20 (llo)] TJ", "(Hello) '", "2 1 (Hello) \""] {
        let mut doc = text_page(&format!("BT /F1 12 Tf 14 TL 72 700 Td {show} [-3000] TJ /F2 12 Tf (ab) Tj ET"));
        let before = text::text_lines(&doc, 0).unwrap();
        assert_eq!(before.len(), 2);
        text::clear_block(&mut doc, 0, 0).unwrap();
        let after = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(after.len(), 1, "{show}");
        assert_eq!(after[0].text, "ab");
        for (a, b) in after[0].rect.iter().zip(before[1].rect) {
            assert!((a - b).abs() < 1e-6, "unselected text moved: {show}");
        }
    }
}

#[test]
fn editor_background_refuses_clipping_text_without_mutation() {
    let mut doc = text_page("BT /F1 12 Tf 7 Tr 72 700 Td (Clip) Tj ET");
    let before = page_content_bytes(&doc, 0);
    assert!(text::clear_block(&mut doc, 0, 0).is_err());
    assert_eq!(page_content_bytes(&doc, 0), before);
}

#[test]
fn explicit_colour_spaces_expose_the_text_colour() {
    let doc = text_page("/DeviceRGB cs 0 0.23 0.45 sc BT /F1 12 Tf 72 700 Td (Blue) Tj ET");
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].color, [0.0, 0.23, 0.45]);
}

#[test]
fn words_in_separate_text_objects_form_one_editable_paragraph() {
    let mut doc = text_page(
        "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hello) Tj ET \
         BT /F1 12 Tf 1 0 0 1 103 700 Tm (world) Tj ET \
         BT /F1 12 Tf 1 0 0 1 72 685 Tm (Second) Tj ET \
         BT /F1 12 Tf 1 0 0 1 117 685 Tm (line) Tj ET \
         BT /F1 12 Tf 1 0 0 1 300 700 Tm (Other column) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].text, "Hello world Second line");
    assert_eq!(blocks[0].lines.len(), 2);
    let other = blocks[1].rect;
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Replacement", "Other column"]);
    assert_eq!(blocks[1].rect, other);
}

#[test]
fn independent_text_objects_with_different_transforms_stay_separate() {
    let doc = text_page(
        "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hello) Tj ET \
         BT /F1 12 Tf 0 1 -1 0 103 700 Tm (Vertical) Tj ET",
    );
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 2);
}

/// A negative font size or `Tz` runs the glyphs backwards along the text matrix: each word's
/// `ink` box still spans all its glyphs, not only the first.
#[test]
fn word_boxes_span_whole_words_when_glyphs_run_backwards() {
    // "someone@example.com" in Helvetica 11 is 121.814 pt wide.
    for content in [
        "BT /F1 -11 Tf -1 0 0 1 300 742 Tm (Mail the team: someone@example.com) Tj ET",
        "BT /F1 11 Tf -100 Tz 300 742 Td (Mail the team: someone@example.com) Tj ET",
    ] {
        let lines = text::text_lines(&text_page(content), 0).unwrap();
        assert_eq!(lines.len(), 1, "{content}");
        let ink = &lines[0].ink;
        assert_eq!(ink.len(), 4, "{content}: {ink:?}");
        let width = |r: &[f64; 4]| r[2] - r[0];
        assert!(ink.iter().all(|r| width(r) > 5.0), "{content}: {ink:?}");
        let address = ink.iter().map(width).fold(0.0, f64::max);
        assert!((address - 121.814).abs() < 0.01, "{content}: {ink:?}");
        let (x0, x1) = ink.iter().fold((f64::MAX, f64::MIN), |(a, b), r| (a.min(r[0]), b.max(r[2])));
        assert!(x0 >= lines[0].rect[0] - 0.01 && x1 <= lines[0].rect[2] + 0.01, "{content}: {ink:?} {:?}", lines[0].rect);
    }
}

/// PDF writers split one font into several subsets, each a font resource of its own: a word drawn
/// through two of them is still one visual line (#3661).
#[test]
fn one_line_continues_across_two_resources_of_the_same_font() {
    for (content, text) in [
        // The issue's case: two resources for the same Helvetica.
        ("BT /F1 12 Tf 72 700 Td (Hel) Tj /F4 12 Tf (lo) Tj ET", "Hello"),
        // Word's case: two tagged subsets of one font, "abcab" through /F2, /F3, /F2.
        ("BT /F2 12 Tf 72 700 Td (ab) Tj /F3 12 Tf (ca) Tj /F2 12 Tf (b) Tj ET", "abcab"),
    ] {
        let doc = text_page(content);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 1, "{content}: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
        assert_eq!(lines[0].text, text, "{content}");
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1, "{content}");
        assert_eq!(blocks[0].text, text, "{content}");
    }
}

/// Two genuinely different fonts still start a new line: the subset tag is all that may differ.
#[test]
fn different_fonts_start_a_new_line() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Hel) Tj /F2 12 Tf (lo) Tj ET");
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 2);
}

/// A font without `/BaseFont` has no name to compare. Its lines still group by resource, as they
/// did before subset joining: an empty name must not split one paragraph into two blocks.
#[test]
fn lines_in_a_font_without_a_base_font_stay_one_block() {
    let doc = text_page("BT /F5 12 Tf 72 700 Td (First) Tj 0 -15 Td (second) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l.base_font.is_empty()), "{:?}", lines.iter().map(|l| &l.base_font).collect::<Vec<_>>());
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "First second");
    let reading = text::reading_blocks(&doc, 0).unwrap();
    assert_eq!(reading.len(), 1, "{:?}", reading.iter().map(|b| &b.text).collect::<Vec<_>>());
    assert_eq!(reading[0].text, "First second");
}

/// Two subsets of one font can carry different encodings: here the second names `-` and `l`
/// through `/Differences` (codes 33 and 34), so the same letters have different codes. The visual
/// line still joins and reads in coding order (#3661).
#[test]
fn a_reencoded_subset_of_the_same_font_stays_one_line() {
    let content = "BT /F1 16 Tf 72 700 Td (Two) Tj /F2 16 Tf (!\") Tj /F1 16 Tf (ine heading) Tj ET";
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
        &stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica-Bold >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /GHIJKL+Helvetica-Bold /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [33 /hyphen /l] >> >>",
    ]);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1, "{:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(lines[0].text, "Two-line heading");
    let reading = text::reading_blocks(&doc, 0).unwrap();
    assert_eq!(reading.len(), 1);
    assert_eq!(reading[0].text, "Two-line heading");
    // Editing the one line replaces every run, including the re-encoded subset's.
    text::replace_line(&mut doc, 0, 0, "Two-line heading done").unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.len(), 1, "{:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(lines[0].text, "Two-line heading done");
}

/// `ABCDE+` is five letters, not the six-letter subset tag of §9.6.4, so it stays part of the
/// name: `ABCDE+Helvetica-Bold` and `Helvetica-Bold` are different fonts and their runs stay apart.
#[test]
fn a_five_letter_prefix_is_not_a_subset_tag() {
    let content = "BT /F1 16 Tf 72 700 Td (Two) Tj /F2 16 Tf (-l) Tj /F1 16 Tf (ine heading) Tj ET";
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
        &stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDE+Helvetica-Bold >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>",
    ]);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 3, "{:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
}

#[test]
fn text_lines_are_found_and_replaced_in_place() {
    let mut doc =
        text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 40 0 Td [(wor) -20 (ld)] TJ 0 -20 Td (Second line) Tj ET BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["Hello world", "Second line", "ab"]);
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01 && lines[0].rect[1] < 700.0 && lines[0].rect[3] > 700.0, "{:?}", lines[0].rect);
    assert!((lines[0].size - 12.0).abs() < 1e-9 && lines[0].base_font == "Helvetica");
    let second = lines[1].rect;
    let r = text::replace_line(&mut doc, 0, 0, "Goodbye, café").unwrap();
    assert_eq!(r.substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].text, "Goodbye, café");
    assert_eq!(lines[1].text, "Second line");
    assert_eq!(lines[1].rect, second);
}

/// A page whose content is one stream in three pieces, split between tokens: a marked-content
/// dictionary closes at the start of the middle piece, and the `TJ` array that ends it is shown
/// by the operator that starts the last one.
fn split_streams_page() -> Document {
    let piece = |s: &str| format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len());
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        piece("/P << /MCID 0"),
        piece(">> BDC BT /F1 12 Tf 72 700 Td (Target) Tj 0 -20 Td [(After) -20 (wards)]"),
        piece("TJ ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
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

/// The page's pieces joined back into one stream: every operator must still have its operands.
fn assert_split_tokens_kept(doc: &Document, new_text: &str) {
    let joined = streams(doc, 0).join("\n");
    assert!(joined.contains(new_text) && !joined.contains("Target"), "{joined}");
    assert!(joined.contains("[(After) -20 (wards)]"), "the text after the line is gone: {joined}");
    for op in pdfcraft_content::parse(joined.as_bytes()).ops {
        let want = match op.op.as_slice() {
            b"BDC" => 2,
            b"TJ" => 1,
            _ => continue,
        };
        assert_eq!(op.operands.len(), want, "{} lost its operands: {joined}", String::from_utf8_lossy(&op.op));
    }
}

#[test]
fn editing_text_keeps_the_tokens_a_stream_shares_with_its_neighbours() {
    let mut doc = split_streams_page();
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Target");
    text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
    assert_split_tokens_kept(&reopen(&doc), "Edited");
    // A paragraph rewrite: "Target" and the line after it (split across pieces) are one
    // paragraph, rewritten whole without stray tokens.
    let mut doc = split_streams_page();
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 1);
    text::replace_block(&mut doc, 0, 0, "Rewrapped").unwrap();
    let (joined, ops) = joined_ops(&reopen(&doc));
    assert!(joined.contains("Rewrapped") && !joined.contains("Target") && !joined.contains("wards"), "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
}

/// The operators of the page's pieces joined back into one stream, each checked for operands.
fn joined_ops(doc: &Document) -> (String, Vec<pdfcraft_content::Op>) {
    let joined = streams(doc, 0).join("\n");
    let ops = pdfcraft_content::parse(joined.as_bytes());
    assert_eq!(ops.skipped, 0, "stray tokens: {joined}");
    (joined, ops.ops)
}

#[test]
fn a_line_whose_operator_starts_the_next_stream_can_be_edited() {
    // #155: `[(After) -20 (wards)]` ends one piece and its `TJ` starts the next.
    let doc = split_streams_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Afterwards"]);
    assert!(lines[1].rect[1] < lines[0].rect[1] && lines[1].rect[2] > lines[1].rect[0] + 40.0, "{:?}", lines[1].rect);

    // Retyping it replaces the operator whole, in both pieces it spans.
    let mut doc = split_streams_page();
    text::replace_line(&mut doc, 0, 1, "Later").unwrap();
    let doc = reopen(&doc);
    let (joined, ops) = joined_ops(&doc);
    assert!(!joined.contains("After") && !joined.contains("wards") && joined.contains("Target"), "{joined}");
    assert!(ops.iter().filter(|o| o.is("TJ")).count() == 0, "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Later"]);
    // The pieces stay pieces: the first is untouched, the marked content still closes.
    let pieces = streams(&doc, 0);
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0].trim(), "/P << /MCID 0");
    assert!(pieces[2].contains("EMC") && !pieces[2].contains("TJ"), "{pieces:?}");

    // As a paragraph with the line before it: both lines are rewritten.
    let mut doc = split_streams_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let last = blocks.iter().position(|b| b.text.contains("Afterwards")).unwrap();
    text::replace_block(&mut doc, 0, last, "Rewrapped").unwrap();
    let doc = reopen(&doc);
    let (joined, ops) = joined_ops(&doc);
    assert!(!joined.contains("wards") && joined.contains("Rewrapped"), "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
    assert!(text::text_lines(&doc, 0).unwrap().iter().any(|l| l.text == "Rewrapped"));
}

#[test]
fn win_ansi_high_glyphs_use_helvetica_not_the_fallback_font() {
    // š ž Š Ž Œ Ÿ ƒ † ‰ (WinAnsi 0x80–0x9F) are standard-font glyphs (#347): no Type3 fallback.
    let text = "Šumava žaba œuvre Ÿ ƒ † ‰ €";
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let r = text::replace_line(&mut doc, 0, 0, text).unwrap();
    assert_eq!(r.substituted.as_deref(), Some("Helvetica"));
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!((lines[0].text.as_str(), lines[0].base_font.as_str()), (text, "Helvetica"));
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let r = text::replace_block(&mut doc, 0, 0, text).unwrap();
    assert!(r.substituted.as_deref().is_none_or(|s| !s.contains("Type3")), "{:?}", r.substituted);
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" "), text);
}

#[test]
fn missing_glyphs_substitute_helvetica_and_impossible_text_is_refused() {
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    // "c" has no glyph in the subset: Helvetica takes over for this line.
    let r = text::replace_line(&mut doc, 0, 0, "abc").unwrap();
    assert_eq!(r.substituted.as_deref(), Some("Helvetica"));
    let doc2 = reopen(&doc);
    let lines = text::text_lines(&doc2, 0).unwrap();
    assert_eq!((lines[0].text.as_str(), lines[0].base_font.as_str()), ("abc", "Helvetica"));
    // Neither the standard font nor the craft-fonts Japanese faces have this emoji.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert!(text::replace_line(&mut doc, 0, 0, "\u{1f4a9}").is_err());
    assert!(text::replace_line(&mut doc, 0, 5, "x").is_err(), "no such line");
}

/// #4280: two paragraphs in subsets without Ç or Ã, one regular and one bold. Each edit that adds
/// an accent sets the paragraph in a standard font of its own weight. Editing the bold one must
/// not turn the regular one, set in its stand-in earlier, bold too, and the editor's frame for the
/// bold one must not reach the regular one.
#[test]
fn a_bold_stand_in_font_leaves_an_earlier_regular_one_alone() {
    let widths = vec!["600"; 26].join(" ");
    let font = |base: &str| {
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /FirstChar 65 /LastChar 90 /Widths [{widths}] /Encoding /WinAnsiEncoding >>")
    };
    let (regular, bold) = (font("ABCDEF+Garamond"), font("ABCDEF+Garamond-Bold"));
    let content = stream("", "BT /F1 12 Tf 72 700 Td (ACAO) Tj ET BT /F2 12 Tf 300 400 Td (TENSAO) Tj ET");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
        &content,
        &regular,
        &bold,
    ]);
    let block = |doc: &Document, text: &str| text::text_blocks(doc, 0).unwrap().iter().position(|b| b.text == text).unwrap();
    let acao = block(&doc, "ACAO");
    assert_eq!(text::replace_block(&mut doc, 0, acao, "AÇÃO").unwrap().substituted.as_deref(), Some("Times-Roman"));
    let first = text::text_lines(&doc, 0).unwrap().into_iter().find(|l| l.text == "AÇÃO").unwrap();

    let tensao = block(&doc, "TENSAO");
    let layout = text::text_edit_layout(&doc, 0, tensao, "TENSÃO", &text::BlockStyle::default()).unwrap();
    let draft = layout.draft_rect.unwrap();
    assert!(draft[3] < first.rect[1] || draft[1] > first.rect[3], "the frame {draft:?} reaches the regular paragraph at {:?}", first.rect);
    assert_eq!(text::replace_block(&mut doc, 0, tensao, "TENSÃO").unwrap().substituted.as_deref(), Some("Times-Bold"));

    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let font_of = |text: &str| lines.iter().find(|l| l.text == text).map(|l| l.base_font.clone());
    assert_eq!(font_of("AÇÃO").as_deref(), Some("Times-Roman"), "{lines:?}");
    assert_eq!(font_of("TENSÃO").as_deref(), Some("Times-Bold"), "{lines:?}");
}

/// #2517: one blank line at ordinary leading (2 × 1.2 × the font size) is a paragraph break.
/// Editing the first paragraph must leave the second one in place.
#[test]
fn a_blank_line_keeps_the_next_paragraph() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (First paragraph, first row.) Tj T* (First paragraph, second row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 656.8 Tm (Second paragraph, first row.) Tj T* (Second paragraph, second row.) Tj ET";
    let mut doc = text_page(src);
    let before = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(before.len(), 2, "{:?}", before.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
    text::replace_block(&mut doc, 0, 0, "First paragraph, edited row.\nFirst paragraph, second row.").unwrap();
    let after = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(after.len(), 2, "{:?}", after.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
    assert!(after[0].text.contains("edited row") && after[0].text.contains("second row"), "{:?}", after[0].text);
    assert!(after[1].text.contains("Second paragraph"), "{:?}", after[1].text);
    // The second row is left in the source stream. A marker around only the rewritten row
    // would split the paragraph; either there is no marker, or it covers both rows and not the next paragraph.
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    if content.contains("/P BMC") {
        let marked = region_through_matching_close(&content, "/P BMC");
        assert!(marked.contains("edited row") && marked.contains("second row"), "marker split the paragraph: {content}");
        assert!(!marked.contains("Second paragraph"), "marker swallowed the next paragraph: {content}");
    }
}

/// #2517: adding a row must not pull in a paragraph that was 3 × the font size below.
#[test]
fn a_grown_paragraph_does_not_swallow_the_next_one() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (One) Tj T* (Two) Tj ET \
               BT /F1 12 Tf 1 0 0 1 72 649.6 Tm (Neighbour) Tj ET";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "One\nTwo\nThree").unwrap();
    let after = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(after.len(), 2, "{:?}", after.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
    assert!(after[1].text.contains("Neighbour"), "{:?}", after[1].text);
}

/// An explicit `TL` is the leading `T*` moves by. Rows at that gap are one paragraph, including
/// when the gap is larger than twice the font size. A following paragraph that starts two
/// leadings lower stays separate. The numbers are the gaps under test, not a claim about how a
/// word processor derives them: this repo's Helvetica box is 0.925 em (ascent 0.718, descent
/// 0.207), so twice that box at 12 pt is 22.2 pt, while 27.6 pt is 12 × 2.3.
#[test]
fn explicit_leading_is_the_measured_baseline_gap() {
    for tl in [14.4_f64, 18.0, 22.2, 24.0, 27.6, 28.8] {
        let src = format!("BT /F1 12 Tf {tl} TL 1 0 0 1 72 700 Tm (One) Tj T* (Two) Tj T* (Three) Tj ET");
        let doc = text_page(&src);
        let lines = text::text_lines(&doc, 0).unwrap();
        let gaps: Vec<f64> = lines.windows(2).map(|w| w[0].origin_baseline() - w[1].origin_baseline()).collect();
        assert!(gaps.iter().all(|gap| (gap - tl).abs() < 0.05), "TL {tl} did not move the baselines: {gaps:?}");
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1, "TL {tl}, measured gaps {gaps:?}: {:?}", blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
    }
    // Two rows at the TL, then a paragraph whose first baseline is two TLs below the second row.
    let tl = 14.4_f64;
    let next = 700.0 - tl - 2.0 * tl;
    let blank = format!(
        "BT /F1 12 Tf {tl} TL 1 0 0 1 72 700 Tm (One) Tj T* (Two) Tj ET \
         BT /F1 12 Tf {tl} TL 1 0 0 1 72 {next} Tm (Neighbour) Tj ET"
    );
    let doc = text_page(&blank);
    let lines = text::text_lines(&doc, 0).unwrap();
    let between = lines[1].origin_baseline() - lines[2].origin_baseline();
    assert!((between - 2.0 * tl).abs() < 0.05, "the blank line measured {between}, not {}", 2.0 * tl);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2, "{:?}", blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
}

/// The neighbour starts one blank line below (two leadings under the second row). Adding a row
/// puts the new baseline one leading above that neighbour, inside the join window. The neighbour
/// still has to survive the edit, both saves, and a second edit.
#[test]
fn growing_into_a_blank_line_keeps_the_neighbour_after_save() {
    let tl = 14.4_f64;
    let src = format!(
        "BT /F1 12 Tf {tl} TL 1 0 0 1 72 700 Tm (One) Tj T* (Two) Tj ET \
         BT /F1 12 Tf {tl} TL 1 0 0 1 72 {} Tm (Neighbour) Tj ET",
        700.0 - 3.0 * tl
    );
    let mut doc = text_page(&src);
    let before = text::text_lines(&doc, 0).unwrap();
    let opening = before[1].origin_baseline() - before[2].origin_baseline();
    assert!((opening - 2.0 * tl).abs() < 0.05, "fixture gap {opening} is not a blank line");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "One\nTwo\nThree").unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    let bases: Vec<f64> = lines.iter().map(|l| l.origin_baseline()).collect();
    assert_eq!(bases.len(), 4, "{bases:?}");
    let grown = bases[2] - bases[3];
    assert!((grown - tl).abs() < 0.05, "the new row is {grown} pt above the neighbour, not one leading");
    assert!(grown < tl * 1.5, "this gap is outside the join window, so the test would pass without the marker");
    let texts = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(texts(&doc), ["One Two Three", "Neighbour"]);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    let marked = region_through_matching_close(&content, "/P BMC");
    for row in ["(One)", "(Two)", "(Three)"] {
        assert!(marked.contains(row), "{row} is outside the marker: {content}");
    }
    assert!(!marked.contains("Neighbour"), "the neighbour was pulled into the marker: {content}");
    for saved in [reopen(&doc), Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()] {
        assert_eq!(texts(&saved), ["One Two Three", "Neighbour"]);
        let mut again = saved;
        text::replace_block(&mut again, 0, 0, "One\nTwo\nFour").unwrap();
        assert_eq!(texts(&reopen(&again)), ["One Two Four", "Neighbour"]);
    }
}

/// No `TL`: `Td` does not set the leading (`TD` does). Three rows at one repeated step are one
/// paragraph. A single large gap, with no third row at that step, stays a break.
#[test]
fn a_repeated_step_without_tl_is_the_paragraph_leading() {
    let step = 27.6_f64;
    let bodies = [
        format!("BT /F1 12 Tf 1 0 0 1 72 700 Tm (One) Tj 0 -{step} Td (Two) Tj 0 -{step} Td (Three) Tj ET"),
        format!(
            "BT /F1 12 Tf 1 0 0 1 72 700 Tm (One) Tj ET BT /F1 12 Tf 1 0 0 1 72 {} Tm (Two) Tj ET BT /F1 12 Tf 1 0 0 1 72 {} Tm (Three) Tj ET",
            700.0 - step,
            700.0 - 2.0 * step
        ),
        format!(
            "BT /F1 12 Tf 72 700 Td (One) Tj ET BT /F1 12 Tf 72 {} Td (Two) Tj ET BT /F1 12 Tf 72 {} Td (Three) Tj ET",
            700.0 - step,
            700.0 - 2.0 * step
        ),
    ];
    for body in bodies {
        let doc = text_page(&body);
        let lines = text::text_lines(&doc, 0).unwrap();
        let gaps: Vec<f64> = lines.windows(2).map(|w| w[0].origin_baseline() - w[1].origin_baseline()).collect();
        assert!(gaps.iter().all(|gap| (gap - step).abs() < 0.05), "step {step} measured {gaps:?} in {body}");
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1, "{body} -> {:?}", blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
    }
    // Two rows and nothing after them: one gap cannot be told from a paragraph break.
    let pair = format!("BT /F1 12 Tf 72 700 Td (One) Tj ET BT /F1 12 Tf 72 {} Td (Two) Tj ET", 700.0 - step);
    assert_eq!(text::text_blocks(&text_page(&pair), 0).unwrap().len(), 2);
    // The original blank line, written with Td instead of TL: the inner step repeats, the break does not.
    let inner = 14.4_f64;
    let blank = format!(
        "BT /F1 12 Tf 72 700 Td (One) Tj ET BT /F1 12 Tf 72 {} Td (Two) Tj ET \
         BT /F1 12 Tf 72 {} Td (Neighbour) Tj ET BT /F1 12 Tf 72 {} Td (Line) Tj ET",
        700.0 - inner,
        700.0 - 3.0 * inner,
        700.0 - 4.0 * inner
    );
    let doc = text_page(&blank);
    let lines = text::text_lines(&doc, 0).unwrap();
    let break_gap = lines[1].origin_baseline() - lines[2].origin_baseline();
    assert!((break_gap - 2.0 * inner).abs() < 0.05, "blank line measured {break_gap}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2, "{:?}", blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>());
}

/// The first row is unchanged, so the rewrite leaves it in the source stream and skips the
/// paragraph marker. The added row then sits one leading above the neighbour.
#[test]
fn a_partial_rewrite_that_adds_a_row_keeps_the_neighbour() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 656.8 Tm (Neighbor stays.) Tj ET";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "Alpha first row.\nTwo\nThree").unwrap();
    let texts = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert_eq!(texts(&doc), ["Alpha first row. Two Three", "Neighbor stays."], "{content}");
    for saved in [reopen(&doc), Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()] {
        assert_eq!(texts(&saved), ["Alpha first row. Two Three", "Neighbor stays."]);
        let mut again = saved;
        text::replace_block(&mut again, 0, 0, "Alpha first row.\nTwo\nFour").unwrap();
        assert_eq!(texts(&reopen(&again)), ["Alpha first row. Two Four", "Neighbor stays."]);
    }
}

/// Both paragraphs share one text object, with a blank line between them. The paragraph
/// marker has to close before the neighbour, not after the shared `ET`.
#[test]
fn a_partial_rewrite_inside_one_text_object_keeps_the_neighbour() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj T* T* (Neighbor stays.) Tj ET";
    let mut doc = text_page(src);
    let texts = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(texts(&doc), ["Alpha first row. Alpha next row.", "Neighbor stays."], "the blank line is a paragraph break");
    text::replace_block(&mut doc, 0, 0, "Alpha first row.\nTwo\nThree").unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert_eq!(texts(&doc), ["Alpha first row. Two Three", "Neighbor stays."], "{content}");
    for saved in [reopen(&doc), Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()] {
        assert_eq!(texts(&saved), ["Alpha first row. Two Three", "Neighbor stays."], "save joined the neighbour");
        let mut again = saved;
        text::replace_block(&mut again, 0, 0, "Alpha first row.\nTwo\nFour").unwrap();
        assert_eq!(texts(&reopen(&again)), ["Alpha first row. Two Four", "Neighbor stays."]);
    }
}

/// The last paragraph of a shared text object has no neighbour after it. The marker still has
/// to open after the preceding paragraph, not at the shared `BT`, or that paragraph joins it.
#[test]
fn a_partial_rewrite_of_the_last_paragraph_in_one_text_object_keeps_the_one_before_it() {
    let src =
        "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj T* T* (Neighbor stays.) Tj T* (Neighbor next.) Tj ET";
    let mut doc = text_page(src);
    let texts = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(texts(&doc), ["Alpha first row. Alpha next row.", "Neighbor stays. Neighbor next."], "the blank line is a paragraph break");
    text::replace_block(&mut doc, 0, 1, "Neighbor stays.\nChanged.").unwrap();
    let content = page_content(&doc);
    assert_eq!(texts(&doc), ["Alpha first row. Alpha next row.", "Neighbor stays. Changed."], "{content}");
    let alpha = content.find("(Alpha first row.)").expect("the preceding paragraph");
    let open = content.find("/P BMC").expect("marker");
    assert!(alpha < open, "the marker swallowed the preceding paragraph: {content}");
    let neighbour = content.find("(Neighbor stays.)").expect("the kept row");
    assert!(open < neighbour, "the kept row is outside the marker: {content}");
    for saved in [reopen(&doc), Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()] {
        assert_eq!(texts(&saved), ["Alpha first row. Alpha next row.", "Neighbor stays. Changed."], "save joined the preceding paragraph");
        let mut again = saved;
        text::replace_block(&mut again, 0, 1, "Neighbor stays.\nEdited.").unwrap();
        assert_eq!(texts(&reopen(&again)), ["Alpha first row. Alpha next row.", "Neighbor stays. Edited."]);
    }
}

/// Three paragraphs share one text object. Rewriting the middle one must leave the paragraph
/// before it and the paragraph after it outside the marker.
#[test]
fn a_partial_rewrite_of_the_middle_paragraph_in_one_text_object_keeps_both_neighbours() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj \
               T* T* (Mid stays.) Tj T* (Mid next.) Tj T* T* (End stays.) Tj T* (End next.) Tj ET";
    let mut doc = text_page(src);
    let texts = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(
        texts(&doc),
        ["Alpha first row. Alpha next row.", "Mid stays. Mid next.", "End stays. End next."],
        "each blank line is a paragraph break"
    );
    text::replace_block(&mut doc, 0, 1, "Mid stays.\nChanged.").unwrap();
    let content = page_content(&doc);
    assert_eq!(texts(&doc), ["Alpha first row. Alpha next row.", "Mid stays. Changed.", "End stays. End next."], "{content}");
    let alpha = content.find("(Alpha first row.)").expect("the preceding paragraph");
    let open = content.find("/P BMC").expect("marker");
    let end = content.find("(End stays.)").expect("the following paragraph");
    assert!(alpha < open, "the marker swallowed the preceding paragraph: {content}");
    assert!(open < end, "the following paragraph is inside the marker: {content}");
    let close = content[open..].find("EMC").map(|at| open + at).expect("close");
    assert!(close < end, "the marker closed after the following paragraph: {content}");
    for saved in [reopen(&doc), Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()] {
        assert_eq!(texts(&saved), ["Alpha first row. Alpha next row.", "Mid stays. Changed.", "End stays. End next."]);
        let mut again = saved;
        text::replace_block(&mut again, 0, 1, "Mid stays.\nEdited.").unwrap();
        assert_eq!(texts(&reopen(&again)), ["Alpha first row. Alpha next row.", "Mid stays. Edited.", "End stays. End next."]);
    }
}

/// #2516's published steps: adding a row must not make the next edit-by-number rewrite Charlie.
/// The issue also asks for a stable paragraph id. That is #2508, and this test does not add one.
#[test]
fn adding_a_row_does_not_make_the_next_paragraph_number_mean_charlie() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha paragraph, first row of text.) Tj T* (Alpha paragraph, second row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 650 Tm (Bravo paragraph, a single row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 600 Tm (Charlie paragraph, first row of text.) Tj T* (Charlie paragraph, second row.) Tj ET";
    let mut doc = text_page(src);
    let names = |doc: &Document| text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(names(&doc).len(), 3, "{:?}", names(&doc));
    text::replace_block(&mut doc, 0, 0, "Alpha paragraph, first row of text.\nAlpha paragraph, second row.\nAlpha paragraph, a third row.").unwrap();
    let after = names(&doc);
    assert!(after.iter().any(|t| t.contains("Bravo") && !t.contains("Alpha")), "Bravo was absorbed: {after:?}");
    assert!(after.iter().any(|t| t.contains("Charlie")), "Charlie is gone: {after:?}");
    let bravo = after.iter().position(|t| t.contains("Bravo")).unwrap();
    text::replace_block(&mut doc, 0, bravo, "Bravo paragraph, edited.").unwrap();
    let done = names(&doc);
    assert!(done.iter().any(|t| t.contains("Charlie")), "editing Bravo removed Charlie: {done:?}");
    assert!(done.iter().any(|t| t.contains("Bravo paragraph, edited.")), "{done:?}");
}

/// The paragraph is the last thing on the page. Editing the first row and leaving the second
/// in the source must keep both rows inside one marker, including after a save.
#[test]
fn a_partial_rewrite_of_the_only_paragraph_stays_one_block() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET";
    let mut doc = text_page(src);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row."], "{texts:?}");
    assert_marker_wraps(&page_content(&doc), "/P BMC", "(One.)", "(Alpha next row.)");
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row."], "{texts:?}");
    assert_marker_wraps(&page_content(&doc), "/P BMC", "(One.)", "(Alpha next row.)");
    let mut doc = doc;
    text::replace_block(&mut doc, 0, 0, "One.\nTwo.").unwrap();
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Two."], "{texts:?}");
    assert_marker_wraps(&page_content(&doc), "/P BMC", "(One.)", "(Two.)");
}

/// The same rewrite when an operator follows the paragraph's `ET`. The close belongs before
/// that operator, and still after the kept row.
#[test]
fn a_partial_rewrite_closes_before_the_operator_that_follows_the_paragraph() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET 0 0 0 RG";
    let mut doc = text_page(src);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row."], "{texts:?}");
    let content = page_content(&doc);
    assert_marker_wraps(&content, "/P BMC", "(One.)", "(Alpha next row.)");
    let start = content.find("/P BMC").expect("marker");
    let close = content[start..].find("EMC").map(|at| start + at).expect("close");
    let after = content.find("0 0 0 RG").expect("the following operator is gone");
    assert!(close < after, "the marker swallowed the operator after the paragraph: {content}");
}

/// The last paragraph on the page is already inside a marked region. A partial rewrite stays
/// one paragraph, and that region's close still comes after both rows.
#[test]
fn a_partial_rewrite_of_the_last_marked_paragraph_stays_inside_its_region() {
    let src = "/H1 << /MCID 0 >> BDC BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET EMC";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 1);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row."], "{texts:?}");
    assert_marker_wraps(&page_content(&doc), "/MCID 0", "(One.)", "(Alpha next row.)");
}

/// A partial rewrite must not insert `/P BMC` across two existing marked-content regions.
/// The neighbour's own marker has to keep wrapping its text.
#[test]
fn a_partial_rewrite_keeps_the_next_marked_region_closed() {
    let src = "/H1 << /MCID 0 >> BDC BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET EMC \
               /P << /MCID 1 >> BDC BT /F1 12 Tf 14.4 TL 1 0 0 1 72 656.8 Tm (Neighbor stays.) Tj ET EMC";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row.", "Neighbor stays."], "{texts:?}");
    let content = page_content(&doc);
    assert_marker_wraps(&content, "/MCID 0", "(One.)", "(Alpha next row.)");
    assert_marker_wraps(&content, "/MCID 1", "(Neighbor stays.)", "(Neighbor stays.)");
    let neighbour = content.find("/MCID 1").expect("neighbour region");
    let close = content[neighbour..].find("EMC").map(|at| neighbour + at).expect("neighbour close");
    assert!(!content[neighbour..close].contains("(One.)"), "the rewrite was pulled into the neighbour's marker: {content}");
}

/// The paragraph being edited is not in a marked region. The next one is. Closing the new
/// marker at the neighbour's `BT` would land after that region's `BDC` and close it early.
#[test]
fn a_partial_rewrite_of_an_unmarked_paragraph_leaves_the_marked_neighbour_closed() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET \
               /P << /MCID 1 >> BDC BT /F1 12 Tf 14.4 TL 1 0 0 1 72 656.8 Tm (Neighbor stays.) Tj ET EMC";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row.", "Neighbor stays."], "{texts:?}");
    let content = page_content(&doc);
    assert_marker_wraps(&content, "/MCID 1", "(Neighbor stays.)", "(Neighbor stays.)");
    assert!(!region_through_matching_close(&content, "/MCID 1").contains("(One.)"), "the rewrite was pulled into the neighbour: {content}");
}

/// Both paragraphs already share one marked region, separated by a blank line. The extra marker
/// around the edit has to close itself, not the region, so the neighbour stays inside it.
#[test]
fn a_partial_rewrite_inside_one_marked_region_leaves_the_neighbour_inside_it() {
    let src = "/P << /MCID 0 >> BDC BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 656.8 Tm (Neighbor stays.) Tj ET EMC";
    let mut doc = text_page(src);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    text::replace_block(&mut doc, 0, 0, "One.\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let texts: Vec<_> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["One. Alpha next row.", "Neighbor stays."], "{texts:?}");
    let content = page_content(&doc);
    let region = region_through_matching_close(&content, "/MCID 0");
    assert!(region.contains("(One.)") && region.contains("(Neighbor stays.)"), "the region closed before the neighbour: {content}");
}

/// Bytes from `open` through the `EMC` that balances the marked region it begins.
fn region_through_matching_close<'a>(content: &'a str, open: &str) -> &'a str {
    let start = content.find(open).unwrap_or_else(|| panic!("missing {open}: {content}"));
    let bytes = content.as_bytes();
    let mut index = start;
    let mut depth = 0i32;
    while index < bytes.len() {
        let rest = &bytes[index..];
        let boundary = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        if boundary && (rest.starts_with(b"BMC") || rest.starts_with(b"BDC")) && rest.get(3).is_none_or(|b| !b.is_ascii_alphanumeric()) {
            depth += 1;
            index += 3;
            continue;
        }
        if boundary && rest.starts_with(b"EMC") && rest.get(3).is_none_or(|b| !b.is_ascii_alphanumeric()) {
            depth -= 1;
            if depth == 0 {
                return &content[start..index];
            }
            index += 3;
            continue;
        }
        index += 1;
    }
    panic!("unbalanced region {open}: {content}");
}

fn page_content(doc: &Document) -> String {
    String::from_utf8_lossy(&page_content_bytes(doc, 0)).into_owned()
}

/// `open` through the next `EMC` contains both strings, and no `EMC` sits inside a text object.
fn assert_marker_wraps(content: &str, open: &str, first: &str, second: &str) {
    let marked = region_through_matching_close(content, open);
    assert!(marked.contains(first) && marked.contains(second), "{open} closed too early: {content}");
    assert!(!marker_crosses_a_text_object(content), "a marker opened outside a text object and closed inside it: {content}");
}

/// A marker may sit inside one text object. It may not open outside one and close inside it.
fn marker_crosses_a_text_object(content: &str) -> bool {
    let mut text = 0i32;
    let mut opened_outside = 0i32;
    let mut opened_inside = 0i32;
    for token in content.split_whitespace() {
        match token {
            "BT" => text += 1,
            "ET" => text -= 1,
            "BMC" | "BDC" => {
                if text == 0 {
                    opened_outside += 1;
                } else {
                    opened_inside += 1;
                }
            }
            "EMC" => {
                if text > 0 {
                    if opened_inside > 0 {
                        opened_inside -= 1;
                    } else {
                        return true;
                    }
                } else if opened_outside > 0 {
                    opened_outside -= 1;
                }
            }
            _ => {}
        }
    }
    text != 0 || opened_outside != 0 || opened_inside != 0
}

/// A blank line is one extra leading, so the fourth row of a grown paragraph sits on the
/// neighbour's baseline. Changing the box width must not treat that neighbour as a duplicate
/// drawing and delete it.
#[test]
fn resizing_a_grown_paragraph_keeps_the_neighbour() {
    let src = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 410 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET \
               BT /F1 12 Tf 14.4 TL 1 0 0 1 72 366.8 Tm (Neighbor stays.) Tj ET";
    let mut doc = text_page(src);
    let text = "Alpha first row.\nthis is another\nalpha test\nAlpha next row.";
    text::replace_block(&mut doc, 0, 0, text).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    let added = lines.iter().find(|l| l.text.contains("Alpha next row.")).unwrap();
    let neighbour = lines.iter().find(|l| l.text.contains("Neighbor")).unwrap();
    assert!((added.origin_baseline() - neighbour.origin_baseline()).abs() < 0.5, "the new row is not on the neighbour");
    let width = text::text_blocks(&doc, 0).unwrap()[0].rect[2] - text::text_blocks(&doc, 0).unwrap()[0].rect[0];
    for factor in [0.6_f64, 1.6] {
        let mut copy = doc.clone();
        text::rewrite_block(&mut copy, 0, 0, Some(text), &text::BlockStyle { width: Some((width * factor).max(40.0)), ..Default::default() })
            .unwrap();
        let texts = text::text_blocks(&copy, 0).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
        assert!(texts.iter().any(|t| t.contains("Neighbor stays.")), "width ×{factor} removed the neighbour: {texts:?}");
    }
}

/// Narrowing wraps a line. Widening it again rejoins that wrap. A newline that was typed stays.
#[test]
fn a_wrapped_line_rejoins_when_widened_and_a_typed_break_stays() {
    let mut doc = text_page("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(45.0), ..Default::default() }).unwrap();
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().next().unwrap();
    assert!(block.editor_text.contains('\n'), "the narrow box did not wrap: {}", block.editor_text);
    assert_eq!(block.reflow_text, "Page 1", "a wrap was kept as a hard break: {}", block.reflow_text);
    text::rewrite_block(&mut doc, 0, 0, Some(&block.reflow_text), &text::BlockStyle { width: Some(200.0), ..Default::default() }).unwrap();
    let lines: Vec<_> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(lines, ["Page 1"]);

    let mut typed = text_page("BT /F1 12 Tf 72 700 Td (One) Tj ET");
    text::replace_block(&mut typed, 0, 0, "One\nTwo\nThree\nfour").unwrap();
    let block = text::text_blocks(&typed, 0).unwrap().into_iter().next().unwrap();
    assert_eq!(block.reflow_text, "One\nTwo\nThree\nfour", "a typed break was rejoined: {}", block.reflow_text);
    let width = (block.rect[2] - block.rect[0]) * 6.0;
    text::rewrite_block(&mut typed, 0, 0, Some(&block.reflow_text), &text::BlockStyle { width: Some(width.max(180.0)), ..Default::default() })
        .unwrap();
    assert_eq!(text::text_blocks(&typed, 0).unwrap()[0].editor_text, "One\nTwo\nThree\nfour");
}

/// #2529 (reviewer): a run of typed spaces that happens to fall at a wrap point is not the same
/// as the single space an ordinary word gap leaves. Narrowing drops the whole gap, as any wrap
/// does (#4199's own test: `wrapped("aa  bb  cc", 6.0) == ["aa  bb", "cc"]`); widening it back
/// must not quietly turn two typed spaces into one. The count is stored on the wrap marker, so
/// it has to survive an incremental save and a full rewrite, not only the in-memory rewrite.
#[test]
fn narrowing_then_widening_keeps_a_typed_double_space() {
    let fresh = text_page("BT /F1 24 Tf 20 150 Td (One  Two) Tj ET");
    let wide_width = {
        let wide = text::text_blocks(&fresh, 0).unwrap().into_iter().next().unwrap().rect;
        wide[2] - wide[0]
    };
    for (save, reopen_as) in
        [("incremental", reopen(&fresh)), ("full", Document::open(Arc::new(write_full(&fresh, &SaveOptions::default()).unwrap())).unwrap())]
    {
        let mut doc = reopen_as;
        text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(45.0), ..Default::default() }).unwrap();
        let doc =
            if save == "incremental" { reopen(&doc) } else { Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap() };
        let block = text::text_blocks(&doc, 0).unwrap().into_iter().next().unwrap();
        assert_eq!(block.editor_text, "One\nTwo", "{save}: the narrow box did not wrap between the two typed spaces: {}", block.editor_text);
        assert_eq!(block.reflow_text, "One  Two", "{save}: the saved wrap did not remember both spaces: {:?}", block.reflow_text);
        let mut doc = doc;
        text::rewrite_block(&mut doc, 0, 0, Some(&block.reflow_text), &text::BlockStyle { width: Some(wide_width), ..Default::default() }).unwrap();
        let doc =
            if save == "incremental" { reopen(&doc) } else { Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap() };
        let rewidened = text::text_blocks(&doc, 0).unwrap().into_iter().next().unwrap();
        assert_eq!(rewidened.editor_text, "One  Two", "{save}: the second typed space was lost: {:?}", rewidened.editor_text);
    }
}

/// #2529: a Chinese/Japanese word split has no space of its own (`wrap`'s own doc comment: "those
/// pieces join without a space"). Narrowing a paragraph until it wraps inside such a word, then
/// widening it back, must restore the two pieces exactly as they were, not insert a space between
/// them. Checked on both the single-style rewrite path and the row-editor (mixed-style) path.
#[test]
fn narrowing_then_widening_a_japanese_word_adds_no_space() {
    if pdfcraft_fonts::document_japanese_font().is_none() {
        eprintln!("skipping narrowing_then_widening_a_japanese_word_adds_no_space: built without craft-fonts (set CRAFT_FONTS_DIR)");
        return;
    }
    let text = "日本語の文章です";
    for draft in [text.to_string(), format!("見出し\n{text}")] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Placeholder text) Tj ET");
        text::rewrite_block(&mut doc, 0, 0, Some(&draft), &text::BlockStyle { width: Some(400.0), ..Default::default() }).unwrap();
        let wide = text::text_blocks(&reopen(&doc), 0).unwrap().into_iter().next().unwrap();
        let wide_width = wide.rect[2] - wide.rect[0];

        // Narrow enough to force a wrap inside the Japanese word, not just between the two rows.
        // A width-only change must pass `reflow_text` explicitly, as the editor UI does: `None`
        // falls back to the paragraph's space-joined display text and would not keep a typed break.
        text::rewrite_block(&mut doc, 0, 0, Some(&wide.reflow_text), &text::BlockStyle { width: Some(60.0), ..Default::default() }).unwrap();
        let narrow = text::text_blocks(&reopen(&doc), 0).unwrap().into_iter().next().unwrap();
        assert!(narrow.rows.len() > draft.matches('\n').count() + 1, "the narrow box did not wrap: {:?}", narrow.editor_text);
        assert_eq!(
            narrow.editor_text.replace('\n', ""),
            draft.replace('\n', ""),
            "wrapping must not add or drop characters: {:?}",
            narrow.editor_text
        );

        text::rewrite_block(&mut doc, 0, 0, Some(&narrow.reflow_text), &text::BlockStyle { width: Some(wide_width), ..Default::default() }).unwrap();
        let rewidened = text::text_blocks(&reopen(&doc), 0).unwrap().into_iter().next().unwrap();
        assert_eq!(rewidened.editor_text, draft, "a Japanese word split gained a space it never had: {:?}", rewidened.editor_text);
    }
}

/// A file that did not mark its breaks still rejoins a line that fills the measure, and keeps a
/// short line that had room.
#[test]
fn an_unmarked_full_line_rejoins_and_a_short_break_stays() {
    let long = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (This sentence is long enough to fill a measure.) Tj ET \
                BT /F1 12 Tf 1 0 0 1 72 685.6 Tm (Tail) Tj ET";
    let doc = text_page(long);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().next().unwrap();
    assert_eq!(block.rows.len(), 2, "{}", block.editor_text);
    assert_eq!(block.reflow_text, "This sentence is long enough to fill a measure. Tail", "{}", block.reflow_text);

    let short = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (One) Tj ET BT /F1 12 Tf 1 0 0 1 72 685.6 Tm (Two) Tj ET";
    let doc = text_page(short);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().next().unwrap();
    assert_eq!(block.reflow_text, "One\nTwo", "{}", block.reflow_text);
}

/// The letter the screenshots edit. Widening example 1, or adding a row there, must leave the
/// heading above it and both paragraphs under example 2 as they were.
fn letter_page() -> &'static str {
    "BT /F1 14 Tf 72 740 Td (1. Repeated step, no TL. Click Two.) Tj ET \
     BT /F1 12 Tf 72 700 Td (One) Tj ET \
     BT /F1 12 Tf 72 672.4 Td (Two) Tj ET \
     BT /F1 12 Tf 72 644.8 Td (Three) Tj ET \
     BT /F1 14 Tf 72 590 Td (2. Blank line. Edit the first paragraph.) Tj ET \
     BT /F1 12 Tf 14.4 TL 1 0 0 1 72 550 Tm (First paragraph, first row.) Tj T* (First paragraph, second row.) Tj ET \
     BT /F1 12 Tf 14.4 TL 1 0 0 1 72 506.8 Tm (Second paragraph, first row.) Tj T* (Second paragraph, second row.) Tj ET \
     BT /F1 14 Tf 72 450 Td (3. Add a row under Alpha. Neighbor stays.) Tj ET \
     BT /F1 12 Tf 14.4 TL 1 0 0 1 72 410 Tm (Alpha first row.) Tj T* (Alpha next row.) Tj ET \
     BT /F1 12 Tf 14.4 TL 1 0 0 1 72 366.8 Tm (Neighbor stays.) Tj ET"
}

fn block_texts(doc: &Document) -> Vec<String> {
    text::text_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect()
}

#[test]
fn widening_example_one_does_not_swallow_the_heading_or_split_the_next_paragraph() {
    let mut doc = text_page(letter_page());
    let before = block_texts(&doc);
    assert!(before.iter().any(|t| t == "1. Repeated step, no TL. Click Two."), "{before:?}");
    assert!(before.iter().any(|t| t.contains("One") && t.contains("Three") && !t.contains("Repeated")), "{before:?}");
    assert!(before.iter().any(|t| t.contains("First paragraph, first row") && t.contains("second row")), "{before:?}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let idx = blocks.iter().position(|b| b.text.contains("One") && b.text.contains("Three")).unwrap();
    // What the editor committed, including the line breaks. A later width change must keep them.
    text::replace_block(&mut doc, 0, idx, "One\nTwo\nThree\nfour").unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let idx = blocks.iter().position(|b| b.text.contains("four") || b.editor_text.contains("four")).unwrap();
    assert_eq!(blocks[idx].reflow_text, "One\nTwo\nThree\nfour", "a typed break was treated as a wrap: {}", blocks[idx].reflow_text);
    let shown = blocks[idx].reflow_text.clone();
    let width = blocks[idx].rect[2] - blocks[idx].rect[0];
    text::rewrite_block(&mut doc, 0, idx, Some(&shown), &text::BlockStyle { width: Some((width * 6.0).max(180.0)), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    let after = block_texts(&doc);
    let kept = text::text_blocks(&doc, 0).unwrap().into_iter().find(|b| b.editor_text.contains("four")).unwrap();
    assert_eq!(kept.editor_text, "One\nTwo\nThree\nfour", "a wider box flattened the line breaks: {:?}", kept.editor_text);
    assert!(after.iter().any(|t| t == "1. Repeated step, no TL. Click Two."), "the heading joined the resized box: {after:?}");
    assert!(
        after.iter().any(|t| t.contains("First paragraph, first row") && t.contains("First paragraph, second row")),
        "widening example 1 split the next paragraph: {after:?}"
    );
    assert!(after.iter().any(|t| t.contains("Second paragraph, first row") && t.contains("second row")), "{after:?}");
    assert!(after.iter().any(|t| t.contains("Neighbor stays.") && !t.contains("Alpha")), "{after:?}");
}

#[test]
fn a_row_added_one_leading_above_the_neighbour_does_not_absorb_it() {
    let mut doc = text_page(letter_page());
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let idx = blocks.iter().position(|b| b.text.contains("Alpha first row")).unwrap();
    text::replace_block(&mut doc, 0, idx, "Alpha first row.\ntest\nAlpha next row.").unwrap();
    let doc = reopen(&doc);
    let after = block_texts(&doc);
    assert!(after.iter().any(|t| t.contains("Neighbor stays.") && !t.contains("Alpha")), "the added row absorbed the neighbour: {after:?}");
}

#[test]
fn paragraphs_are_found_and_rewrapped() {
    let mut doc = text_page(
        "BT 0 0 1 rg /F1 10 Tf 12 TL 72 700 Td (The quick brown fox) Tj T* (jumps over the) Tj T* (lazy dog.) Tj ET \
         BT /F1 10 Tf 72 600 Td (Next paragraph) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["The quick brown fox jumps over the lazy dog.", "Next paragraph"]);
    assert_eq!(blocks[0].lines, [0, 1, 2]);
    let width = blocks[0].rect[2] - blocks[0].rect[0];
    let next = blocks[1].rect;
    let long = "PdfCraft rewraps a paragraph to its own width when its text changes, keeping the font, size, colour and line spacing.";
    assert_eq!(text::replace_block(&mut doc, 0, 0, long).unwrap().substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, long);
    assert!(blocks[0].lines.len() > 3, "more lines: {}", blocks[0].lines.len());
    for i in &blocks[0].lines {
        assert!(lines[*i].rect[2] - lines[*i].rect[0] <= width + 1.0, "line {} fits", lines[*i].text);
    }
    // Same left edge, same spacing (12 pt).
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01);
    let spacing = lines[blocks[0].lines[0]].origin_baseline() - lines[blocks[0].lines[1]].origin_baseline();
    assert!((spacing - 12.0).abs() < 0.01, "{spacing}");
    // The next paragraph is untouched, and the new text is blue like the old.
    assert_eq!(blocks[1].text, "Next paragraph");
    assert_eq!(blocks[1].rect, next);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("0 0 1 rg"), "{content}");
    // Shorter text: fewer lines.
    let mut doc = doc;
    text::replace_block(&mut doc, 0, 0, "Short.").unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!((blocks[0].text.as_str(), blocks[0].lines.len()), ("Short.", 1));
}

/// A list whose bullet is its own text object at the margin and whose text sits at a hanging
/// indent: the wrapped first item is one paragraph, and editing it keeps the items after it
/// (#2564, the edit that deleted its neighbours).
#[test]
fn a_wrapped_list_item_keeps_its_continuation_and_the_items_after_it() {
    let mut doc = text_page(
        "BT /F1 12 Tf 1 0 0 1 72 700 Tm (\\225) Tj ET \
         BT /F1 12 Tf 14.4 TL 1 0 0 1 86 700 Tm (First item of the list that wraps) Tj T* (onto a second row.) Tj ET \
         BT /F1 12 Tf 1 0 0 1 72 671.2 Tm (\\225) Tj ET \
         BT /F1 12 Tf 14.4 TL 1 0 0 1 86 671.2 Tm (Second item, one row.) Tj ET \
         BT /F1 12 Tf 1 0 0 1 72 656.8 Tm (\\225) Tj ET \
         BT /F1 12 Tf 14.4 TL 1 0 0 1 86 656.8 Tm (Third item, one row.) Tj ET",
    );
    let items = ["• First item of the list that wraps onto a second row.", "• Second item, one row.", "• Third item, one row."];
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, items, "one paragraph per item, the continuation with its first row");
    // Editing the paragraph the wrapped row belongs to must not take the items after it with it.
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let wrapped = blocks.iter().position(|b| b.text.contains("onto a second row.")).unwrap();
    text::replace_block(&mut doc, 0, wrapped, "• First item of the list that wraps onto a second row, edited.").unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(
        texts,
        ["• First item of the list that wraps onto a second row, edited.", "• Second item, one row.", "• Third item, one row."],
        "the items after the edited one are still on the page"
    );
}

fn page_content_bytes(doc: &Document, page: usize) -> Vec<u8> {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let c = p.dict.get(b"Contents").unwrap();
    match &*doc.resolve(c) {
        pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
        pdfcraft_cos::Object::Array(a) => a
            .iter()
            .flat_map(|x| match &*doc.resolve(x) {
                pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A page drawing image XObject /Im0 at 100,100 size 200 × 100 (pixels 4 × 2).
fn image_page() -> Document {
    image_page_with("q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET")
}

fn image_page_with(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 8 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\nendstream".into(),
    ];
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

/// A path segment operator before any `m` (or `re`) has no subpath to extend: the image scan used
/// to call `BezPath`'s `line_to`/`curve_to`/`close_path` on an empty path, which panics by contract,
/// taking `object_list`, `page_images` and the Office export's image walk down with it (#3581).
/// The segment is now skipped, as the renderer, MuPDF and pdftotext do, and the page's images are
/// still listed.
#[test]
fn image_scan_skips_a_path_segment_before_any_m() {
    for bad in ["1 1 l S", "1 1 2 2 3 3 c S", "1 1 2 2 4 4 y S", "1 1 2 2 3 3 v S", "h S"] {
        let content = format!("q {bad} Q 100 100 200 100 cm /Im0 Do");
        let doc = image_page_with(&content);
        assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1, "{bad}: the image is still listed");
    }
    // A path started with m (or re) still extends and closes as before.
    let doc = image_page_with("q 0 0 m 1 1 l h S Q 100 100 200 100 cm /Im0 Do");
    assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1);
    let doc = image_page_with("q 10 10 20 20 re W n Q 100 100 200 100 cm /Im0 Do");
    assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1);
}

/// The box of the first `re` on the page in user space, after the `cm`s before it.
fn first_rect_in_user_space(doc: &Document) -> [f64; 4] {
    let mut ctm = pdfcraft_content::Matrix::IDENTITY;
    for op in pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops {
        if op.is("cm") {
            ctm = pdfcraft_content::Matrix(op.nums::<6>().unwrap()).then(&ctm);
        } else if op.is("re") {
            let [x, y, w, h] = op.nums::<4>().unwrap();
            return ctm.bbox([x, y, x + w, y + h]);
        }
    }
    panic!("no clip rectangle");
}

#[test]
fn moving_an_image_moves_its_own_clip_too() {
    let content = "q 100 100 200 100 re W* n 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET";
    let target = [50.0, 400.0, 350.0, 600.0];
    let mut doc = image_page_with(content);
    let before = images::page_images(&doc, 0).unwrap()[0].rect;
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(before, target))).unwrap();
    let mut doc = reopen(&doc);
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, target));
    assert!(close(first_rect_in_user_space(&doc), target), "{:?}", first_rect_in_user_space(&doc));
    // A second edit still finds the clip, now behind the `cm` the first one put before it.
    let moved = [80.0, 420.0, 380.0, 620.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(target, moved))).unwrap();
    assert!(close(first_rect_in_user_space(&doc), moved));
    images::translate_images(&mut doc, 0, &[0], [10.0, 0.0]).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [90.0, 420.0, 390.0, 620.0]));
    assert!(close(first_rect_in_user_space(&doc), [90.0, 420.0, 390.0, 620.0]));
    // Moving it with the group move helper takes the clip along as well.
    let mut doc = image_page_with(content);
    images::translate_images(&mut doc, 0, &[0], [30.0, -20.0]).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [130.0, 80.0, 330.0, 180.0]));
    assert!(close(first_rect_in_user_space(&doc), [130.0, 80.0, 330.0, 180.0]));
    // A clip shared with other drawing is left where it is.
    let shared = "q 100 100 200 100 re W n q 200 0 0 100 100 100 cm /Im0 Do Q 0 0 10 10 re f Q";
    let mut doc = image_page_with(shared);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(before, target))).unwrap();
    assert!(close(first_rect_in_user_space(&doc), [100.0, 100.0, 300.0, 200.0]));
}

fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

/// The deepest `q` nesting on the page.
fn q_depth(doc: &Document) -> usize {
    let mut depth = 0_usize;
    let mut deepest = 0;
    for op in pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops {
        if op.is("q") {
            depth += 1;
            deepest = deepest.max(depth);
        } else if op.is("Q") {
            depth = depth.saturating_sub(1);
        }
    }
    deepest
}

#[test]
fn repeated_image_edits_change_one_transform_instead_of_nesting() {
    let variants = [
        // Its own placement, a private crop, and a bare `Do` that the first edit wraps.
        "q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET",
        "q 100 100 200 100 re W* n 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET",
        "200 0 0 100 100 100 cm /Im0 Do",
    ];
    for content in variants {
        let mut doc = image_page_with(content);
        let resize = |doc: &mut Document, n: usize| -> usize {
            let mut depth = 0;
            for i in 0..n {
                let r = images::page_images(doc, 0).unwrap()[0].visible_rect.unwrap();
                // Grow by 3% about the lower-left corner and move right by 2, staying on the page.
                let target = [r[0] + 2.0, r[1], r[0] + 2.0 + (r[2] - r[0]) * 1.03, r[1] + (r[3] - r[1]) * 1.03];
                images::change_image(doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(r, target))).unwrap();
                let after = images::page_images(doc, 0).unwrap()[0].visible_rect.unwrap();
                assert!(after.iter().zip(target).all(|(a, b)| (a - b).abs() < 1e-6), "{content}: {after:?} vs {target:?}");
                if i == 0 {
                    depth = q_depth(doc);
                }
            }
            depth
        };
        let first = resize(&mut doc, 20);
        assert_eq!(q_depth(&doc), first, "{content}: twenty resizes nest no deeper than one");
        let mut doc = reopen(&doc);
        let before = images::page_images(&doc, 0).unwrap()[0].rect;
        for _ in 0..20 {
            images::translate_images(&mut doc, 0, &[0], [1.0, -2.0]).unwrap();
        }
        let after = images::page_images(&doc, 0).unwrap()[0].rect;
        assert!(close(after, [before[0] + 20.0, before[1] - 40.0, before[2] + 20.0, before[3] - 40.0]), "{content}: {after:?}");
        assert_eq!(q_depth(&doc), first, "{content}: group moves change the same transform");
        let caption: Vec<_> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
        assert_eq!(caption.is_empty(), !content.contains("Caption"), "{content}: the text after the image is untouched");
    }
}

#[test]
fn repeated_group_moves_of_a_cropped_image_change_one_clip_transform() {
    // The crop is smaller than the image, so a clip left behind would shrink `visible_rect`.
    let mut doc = image_page_with("q 150 120 100 50 re W n 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET");
    let operators = |doc: &Document| pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops.len();
    let start = images::page_images(&doc, 0).unwrap()[0].clone();
    let mut first = 0;
    for i in 0..20 {
        images::translate_images(&mut doc, 0, &[0], [1.0, -2.0]).unwrap();
        if i == 0 {
            first = operators(&doc);
        }
    }
    let moved = |r: [f64; 4]| [r[0] + 20.0, r[1] - 40.0, r[2] + 20.0, r[3] - 40.0];
    let now = images::page_images(&doc, 0).unwrap()[0].clone();
    assert!(close(now.rect, moved(start.rect)), "{:?}", now.rect);
    assert!(close(now.visible_rect.unwrap(), moved(start.visible_rect.unwrap())), "the crop moves with the image: {:?}", now.visible_rect);
    assert_eq!(operators(&doc), first, "twenty group moves add no more operators than one");
    assert_eq!(q_depth(&doc), 1);
    let doc = reopen(&doc);
    assert!(close(images::page_images(&doc, 0).unwrap()[0].visible_rect.unwrap(), moved(start.visible_rect.unwrap())));
    let caption: Vec<_> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(caption, ["Caption"]);
}

#[test]
fn page_images_move_turn_replace_and_delete() {
    let mut doc = image_page();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!(imgs.len(), 1);
    assert!(close(imgs[0].rect, [100.0, 100.0, 300.0, 200.0]), "{:?}", imgs[0].rect);
    assert_eq!((imgs[0].width, imgs[0].height, imgs[0].name.as_str()), (4, 2, "Im0"));
    // Move and resize.
    let t = images::rect_to_rect(imgs[0].rect, [50.0, 400.0, 150.0, 450.0]);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let doc = reopen(&doc);
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [50.0, 400.0, 150.0, 450.0]), "{r:?}");
    // A quarter turn about the centre: 100 × 50 becomes 50 × 100 around (100, 425).
    let mut doc = doc;
    let t = images::turn_about_centre(r, 1, false, false);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [75.0, 375.0, 125.0, 475.0]), "{r:?}");
    // The text after it is untouched.
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Caption");
    // Replace with another image object, in the same place.
    let mut d = pdfcraft_cos::Dict::new();
    for (k, v) in [
        (&b"Type"[..], pdfcraft_cos::Object::name("XObject")),
        (b"Subtype", pdfcraft_cos::Object::name("Image")),
        (b"Width", pdfcraft_cos::Object::Int(1)),
        (b"Height", pdfcraft_cos::Object::Int(1)),
    ] {
        d.set(k.to_vec(), v);
    }
    let other = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(d, vec![0])));
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Replace(other)).unwrap();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!((imgs[0].object, imgs[0].width), (Some(other), 1));
    assert!(close(imgs[0].rect, r));
    // Delete.
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).unwrap();
    assert!(images::page_images(&doc, 0).unwrap().is_empty());
    assert!(images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).is_err());
}

#[test]
fn form_artwork_edits_only_the_selected_placement() {
    let mut doc = fixture(); // three pages share resources; pages 0 and 1 also share contents
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([10, 20, 110, 70].map(Object::Int).to_vec()));
    form.set(b"Matrix".to_vec(), Object::Array([2, 0, 0, 3, -20, -60].map(Object::Int).to_vec()));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"1 0 0 rg 10 20 100 50 re f")));
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |d| {
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        d.set(b"XObject".to_vec(), Object::Dict(xo));
    })
    .unwrap();
    let content = b"q 1 0 0 1 40 80 cm /Figure Do Q q 1 0 0 1 300 400 cm /Figure Do Q BT /F1 9 Tf (Caption) Tj ET";
    doc.set(pdfcraft_cos::ObjRef::new(7, 0), Object::Stream(Stream::flate(Dict::new(), content)));
    let mut doc = reopen(&doc);
    let original = doc.get(artwork);
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!(imgs.len(), 2);
    assert!(imgs[0].is_form);
    assert_eq!((imgs[0].width, imgs[0].height), (0, 0));
    assert!(close(imgs[0].rect, [40.0, 80.0, 240.0, 230.0]));
    let target = [60.0, 100.0, 160.0, 175.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(imgs[0].rect, target))).unwrap();
    let mut doc = reopen(&doc);
    let moved = images::page_images(&doc, 0).unwrap();
    assert!(close(moved[0].rect, target));
    assert_eq!(moved[1].rect, imgs[1].rect);
    assert_eq!(images::page_images(&doc, 1).unwrap()[0].rect, imgs[0].rect);
    assert_eq!(*doc.get(artwork), *original, "the shared Form and its unknown data are untouched");
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Caption");
    let t = images::turn_about_centre(target, 1, false, false);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [72.5, 87.5, 147.5, 187.5]));
    assert!(images::change_image(&mut doc, 0, 0, &images::ImageChange::Replace(artwork)).is_err());
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).unwrap();
    let doc = reopen(&doc);
    assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1);
    assert_eq!(images::page_images(&doc, 1).unwrap().len(), 2);
    // Invalid bounds are skipped without interpreting the Form stream.
    let mut doc = doc;
    let mut broken = (*doc.get(artwork)).clone();
    if let Object::Stream(s) = &mut broken {
        s.dict.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0)]));
    }
    doc.set(artwork, broken);
    assert!(images::page_images(&doc, 0).unwrap().is_empty());
}

#[test]
fn paragraphs_take_new_formatting() {
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let style = text::BlockStyle {
        family: Some((added::Family::Times, true, false)),
        size: Some(12.0),
        color: Some([1.0, 0.0, 0.0]),
        align: Some(added::Align::Center),
        ..Default::default()
    };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, before.text, "same words");
    assert_eq!((blocks[0].base_font.as_str(), blocks[0].size), ("Times-Bold", 12.0));
    // Centred in the paragraph's width.
    let mid = (before.rect[0] + before.rect[2]) / 2.0;
    for i in &blocks[0].lines {
        let r = lines[*i].rect;
        assert!(((r[0] + r[2]) / 2.0 - mid).abs() < 2.0, "line {:?} centred on {mid}", lines[*i].text);
    }
    assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("1 0 0 rg"));
    // Right alignment keeps lines flush with the right edge.
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Right), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
}

#[test]
fn a_one_row_title_keeps_its_centre_when_it_grows_and_when_it_wraps() {
    // A centred one-row title (its centre is at x ≈ 306 on the 600 pt page) above a body
    // left-aligned at x = 72. Growing it must not drag it to the right.
    let src = "BT /F1 18 Tf 1 0 0 1 252.477 700 Tm (Centered title) Tj ET\n\
               BT /F1 12 Tf 1 0 0 1 72 640 Tm (Body text below the heading starts here and runs) Tj ET\n\
               BT /F1 12 Tf 1 0 0 1 72 625.6 Tm (on for a second row of ordinary words.) Tj ET";
    let mut doc = text_page(src);
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let centre = (before.rect[0] + before.rect[2]) / 2.0;
    text::rewrite_block(&mut doc, 0, 0, Some("Centered title with a few more words"), &text::BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, "Centered title with a few more words");
    let r = lines[blocks[0].lines[0]].rect;
    assert!(((r[0] + r[2]) / 2.0 - centre).abs() < 1.0, "the grown title stays centred on {centre}, its centre is {}", (r[0] + r[2]) / 2.0);
    // Long enough to have to wrap: every row stays centred on the same centre too.
    let mut doc = text_page(src);
    text::rewrite_block(
        &mut doc,
        0,
        0,
        Some("Centered title with many more words added so that the single row has to become much longer"),
        &text::BlockStyle::default(),
    )
    .unwrap();
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(blocks[0].lines.len() > 1, "the long title wraps into rows: {:?}", blocks[0].text);
    for i in &blocks[0].lines {
        let r = lines[*i].rect;
        assert!(((r[0] + r[2]) / 2.0 - centre).abs() < 2.0, "row {:?} stays centred on {centre}", lines[*i].text);
    }
}

#[test]
fn a_new_paragraph_colour_does_not_spill_into_the_text_after_it() {
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj ET");
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    // The fill colour in force where the second paragraph is shown (q/Q nest it).
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut fill, mut stack) = (String::from("0 g"), Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"q" => stack.push(fill.clone()),
            b"Q" => fill = stack.pop().unwrap_or_default(),
            b"g" | b"rg" | b"k" => fill = String::from_utf8_lossy(&pdfcraft_content::serialize_ops(std::slice::from_ref(op))).trim().to_string(),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Second paragraph") => break,
            _ => {}
        }
    }
    assert_eq!(fill, "0 g", "{ops:?}");
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[1].text, "Second paragraph");
}

#[test]
fn a_rewritten_paragraph_keeps_its_place_in_a_shared_text_object() {
    // Three paragraphs in one BT … ET: rewriting the second keeps the order (paragraph numbers
    // and reading order) and every paragraph's position.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let before = text::text_blocks(&doc, 0).unwrap();
    text::replace_block(&mut doc, 0, 1, "Edited second").unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = after.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["First paragraph", "Edited second", "Third paragraph"]);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[2].rect, before[2].rect);
    assert!((after[1].rect[0] - before[1].rect[0]).abs() < 0.01 && (after[1].rect[1] - before[1].rect[1]).abs() < 0.01, "{:?}", after[1].rect);
}

#[test]
fn recolouring_a_paragraph_in_a_shared_text_object_keeps_order_and_nesting() {
    // Splitting the text object (to keep the order) and q … Q (to keep the colour in) together:
    // q/Q stay outside text objects, and the paragraph after it keeps the original colour.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 1, None, &style).unwrap();
    let doc = reopen(&doc);
    let texts: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["First paragraph", "Second paragraph", "Third paragraph"]);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut in_text, mut depth, mut red) = (false, 0usize, Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"q" | b"Q" => {
                assert!(!in_text, "q/Q inside a text object: {ops:?}");
                depth = if op.is("q") { depth + 1 } else { depth.saturating_sub(1) };
            }
            b"rg" => red.push(depth),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Third paragraph") => {
                assert!(red.iter().all(|d| *d > depth), "red still in force for the third paragraph: {ops:?}");
            }
            _ => {}
        }
    }
    assert!(!red.is_empty(), "{ops:?}");
}

#[test]
fn justify_underline_and_spacing() {
    let src = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj T* (end) Tj ET";
    // Justified: every line but the last reaches the right edge.
    let mut doc = text_page(src);
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
    // Double line spacing, underlined.
    let mut doc = text_page(src);
    let style = text::BlockStyle { line_spacing: Some(2.0), underline: Some(true), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!((lines[0].origin_baseline() - lines[1].origin_baseline() - 20.0).abs() < 0.01);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains(" l\nS\n") || content.contains(" l S"), "{content}");
    // Character spacing widens, horizontal scale narrows.
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    let w0 = text::text_lines(&doc, 0).unwrap()[0].rect;
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { char_spacing: Some(2.0), ..Default::default() }).unwrap();
    let w1 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!((w1[2] - w1[0]) - (w0[2] - w0[0]) > 15.0, "{w0:?} → {w1:?}");
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { scale: Some(50.0), ..Default::default() }).unwrap();
    let w2 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!(((w2[2] - w2[0]) - (w0[2] - w0[0]) / 2.0).abs() < 1.0, "{w0:?} → {w2:?}");
}

#[test]
fn a_line_drawn_twice_is_replaced_everywhere() {
    // Fake bold: a line drawn twice at the same spot (each copy its own text object). The two
    // copies group apart, but editing one replaces both — a surviving copy would show the old
    // text under the new.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "the copies group apart");
    text::replace_block(&mut doc, 0, 0, "Hello world").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello world"], "one line, replaced");
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert_eq!(content.matches("(Hello").count(), 1, "the second copy's operators are gone: {content}");
    // Copies in one text object (the matrix re-issued at the same spot) go too.
    let mut doc = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hi) Tj 1 0 0 1 72.3 700 Tm (Hi) Tj ET");
    text::replace_line(&mut doc, 0, 0, "Hey").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hey"], "one line, replaced");
    // A neighbouring line is not coincident and stays (40 pt spacing groups apart at 12 pt).
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 0 -40 Td (world) Tj ET");
    text::replace_block(&mut doc, 0, 0, "Hello there").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello there", "world"]);
}

#[test]
fn replacing_a_line_keeps_a_distinct_nearby_line() {
    // 12 pt lines 6 pt apart: the glyph boxes overlap by more than half the smaller box,
    // but the baselines are different lines (#1316).
    let src = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -6 Td (Beta) Tj ET";
    let listed = |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().map(|l| l.text).collect::<Vec<_>>();
    for paragraph in [false, true] {
        let mut doc = text_page(src);
        assert_eq!(listed(&doc), ["Alpha", "Beta"]);
        assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
        if paragraph {
            text::rewrite_block(&mut doc, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, "ALPHA").unwrap();
        }
        assert_eq!(listed(&doc), ["ALPHA", "Beta"]);
        assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("(Beta)"));
        for bytes in [write_incremental(&doc, &SaveOptions::default()).unwrap(), write_full(&doc, &SaveOptions::default()).unwrap()] {
            let back = Document::open(Arc::new(bytes)).unwrap();
            assert!(back.repair_log().is_empty());
            assert_eq!(listed(&back), ["ALPHA", "Beta"]);
        }
    }
    // The same word on the next line is still a different line.
    let mut same = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -6 Td (Alpha) Tj ET");
    text::replace_line(&mut same, 0, 0, "ALPHA").unwrap();
    assert_eq!(listed(&same), ["ALPHA", "Alpha"]);
    // Ordinary leading, and a paragraph far enough to group apart, keep the neighbour.
    let mut spaced = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -14.4 Td (Beta) Tj ET");
    text::replace_line(&mut spaced, 0, 0, "ALPHA").unwrap();
    assert_eq!(listed(&spaced), ["ALPHA", "Beta"]);
    let mut apart = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -40 Td (Beta) Tj ET");
    text::rewrite_block(&mut apart, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
    assert_eq!(listed(&apart), ["ALPHA", "Beta"]);
    // A second copy at the same position is still one replacement.
    for paragraph in [false, true] {
        let mut duplicate = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj ET BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj ET");
        if paragraph {
            text::rewrite_block(&mut duplicate, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
        } else {
            text::replace_line(&mut duplicate, 0, 0, "ALPHA").unwrap();
        }
        assert_eq!(listed(&duplicate), ["ALPHA"]);
    }
}

#[test]
fn editing_text_keeps_an_overlapping_watermark_and_covered_paragraphs() {
    let listed = |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().map(|l| l.text).collect::<Vec<_>>();
    // #1756: a rotated watermark's bounding box covers the middle of the page, and a body line
    // whose baseline happens to sit near the watermark's starting y is still unrelated text.
    for rotation in [35.0, 45.0, 0.0] {
        let mut base = text_page("BT /F1 14 Tf 60 700 Td (Heading) Tj ET");
        add_watermark(
            &mut base,
            &[0],
            &Watermark { text: "TEST WATERMARK".into(), font_size: 48.0, rotation, opacity: 0.25, ..Watermark::default() },
            false,
        )
        .unwrap();
        let wm_y = text::text_lines(&base, 0).unwrap().into_iter().find(|l| l.text == "TEST WATERMARK").unwrap().origin_baseline();
        let mut doc = text_page(&format!("BT /F1 14 Tf 1 0 0 1 60 {wm_y:.2} Tm (Body paragraph one with several words here) Tj ET"));
        add_watermark(
            &mut doc,
            &[0],
            &Watermark { text: "TEST WATERMARK".into(), font_size: 48.0, rotation, opacity: 0.25, ..Watermark::default() },
            false,
        )
        .unwrap();
        let line_idx = text::text_lines(&doc, 0).unwrap().iter().position(|l| l.text.starts_with("Body")).unwrap();
        let mut line_edited = doc.clone();
        text::replace_line(&mut line_edited, 0, line_idx, "Edited body line").unwrap();
        assert_eq!(listed(&reopen(&line_edited)), ["Edited body line", "TEST WATERMARK"], "rotation {rotation}");

        let block_idx = text::text_blocks(&doc, 0).unwrap().iter().position(|b| b.text.starts_with("Body")).unwrap();
        let mut preview = doc.clone();
        text::clear_block(&mut preview, 0, block_idx).unwrap();
        assert_eq!(listed(&preview), ["TEST WATERMARK"], "clear_block keeps the watermark at rotation {rotation}");

        text::rewrite_block(&mut doc, 0, block_idx, Some("Edited body paragraph"), &text::BlockStyle::default()).unwrap();
        assert_eq!(listed(&reopen(&doc)), ["Edited body paragraph", "TEST WATERMARK"], "rotation {rotation}");
    }

    // #1368: dragging one paragraph onto another paragraph's baseline and moving it on again
    // leaves the covered paragraph intact.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (First paragraph) Tj ET BT /F1 12 Tf 72 600 Td (Second paragraph) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([0.0, -100.0]), ..Default::default() }).unwrap();
    let second = text::text_blocks(&doc, 0).unwrap().iter().position(|b| b.text == "Second paragraph");
    assert!(second.is_some(), "the covered paragraph stays on the page: {:?}", listed(&doc));
    let first = text::text_blocks(&doc, 0).unwrap().iter().position(|b| b.text == "First paragraph").unwrap();
    text::rewrite_block(&mut doc, 0, first, None, &text::BlockStyle { offset: Some([0.0, -100.0]), ..Default::default() }).unwrap();
    assert_eq!(listed(&reopen(&doc)), ["First paragraph", "Second paragraph"]);

    // Even when a large horizontal watermark shows the same word as a body line at the same y,
    // or a rotated 12 pt line crosses a horizontal 12 pt line of the same word at the same origin,
    // editing one keeps the other.
    let mut same_word = text_page("BT /F1 12 Tf 1 0 0 1 200 400 Tm (DRAFT) Tj ET BT /F1 48 Tf 1 0 0 1 150 400 Tm (DRAFT) Tj ET");
    text::replace_line(&mut same_word, 0, 0, "FINAL").unwrap();
    let after = text::text_lines(&reopen(&same_word), 0).unwrap();
    assert_eq!(after.iter().map(|l| (&l.text[..], l.size as i32)).collect::<Vec<_>>(), [("FINAL", 12), ("DRAFT", 48)]);

    let mut crossed = text_page("BT /F1 12 Tf 1 0 0 1 100 500 Tm (Crossed) Tj ET BT /F1 12 Tf 0.707 0.707 -0.707 0.707 100 500 Tm (Crossed) Tj ET");
    text::replace_line(&mut crossed, 0, 0, "Horizontal").unwrap();
    assert_eq!(listed(&reopen(&crossed)), ["Horizontal", "Crossed"]);

    // An invisible OCR text layer (`3 Tr`) on the same baseline and size as a visible line is
    // still removed when the visible line is edited, even if OCR misread a letter.
    let mut ocr = text_page("BT /F1 12 Tf 0 Tr 1 0 0 1 72 700 Tm (Hello world) Tj ET BT /F1 12 Tf 3 Tr 1 0 0 1 72 700 Tm (Rello world) Tj ET");
    text::replace_line(&mut ocr, 0, 0, "Goodbye world").unwrap();
    assert_eq!(listed(&reopen(&ocr)), ["Goodbye world"]);
}

#[test]
fn paragraphs_move_and_rewrap_to_a_new_width() {
    let para = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET";
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Moved 50 right and 200 down, same words and wrapping.
    let mut doc = text_page(para);
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([50.0, -200.0]), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap()[0].clone();
    assert_eq!((after.text.as_str(), after.lines.len()), (before.text.as_str(), before.lines.len()));
    assert!(close(after.rect[0], before.rect[0] + 50.0) && close(after.rect[1], before.rect[1] - 200.0), "{:?} → {:?}", before.rect, after.rect);
    // Drawn at half scale: the move is still in page space.
    let mut doc = text_page(&format!("q 0.5 0 0 0.5 0 0 cm {} Q", para.replace("100 700 Td", "200 1400 Td")));
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([30.0, 40.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(close(after.rect[0], before.rect[0] + 30.0) && close(after.rect[1], before.rect[1] + 40.0), "{:?} → {:?}", before.rect, after.rect);
    // A narrower box rewraps into more lines that fit it; a wider one into fewer.
    let mut doc = text_page(para);
    let left = text::text_blocks(&doc, 0).unwrap()[0].rect[0];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(80.0), ..Default::default() }).unwrap();
    let narrow = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(narrow.lines.len() > 2, "{narrow:?}");
    assert!(narrow.rect[2] <= left + 80.5, "{narrow:?}");
    let mut doc = text_page(para);
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(400.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].lines.len(), 1);
    // Untrusted numbers (the automation tools pass them through) are refused, not drawn.
    let mut doc = text_page(para);
    for style in [
        text::BlockStyle { offset: Some([f64::NAN, 0.0]), ..Default::default() },
        text::BlockStyle { offset: Some([0.0, f64::INFINITY]), ..Default::default() },
        text::BlockStyle { width: Some(f64::NAN), ..Default::default() },
    ] {
        assert!(text::rewrite_block(&mut doc, 0, 0, None, &style).is_err(), "{style:?}");
    }
    // A zero width is clamped to one character's width rather than looping or vanishing.
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(0.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap().iter().map(|b| b.text.split_whitespace().count()).sum::<usize>(), 12);
}

/// A page split into several content streams, as AutoCAD writes them: the first scales the
/// page (`0.12 0 0 0.12 0 0 cm`, no `q`), the text comes in a later one.
fn split_page(first: &str, second: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{first}\nendstream", first.len()),
        format!("<< /Length {} >>\nstream\n{second}\nendstream", second.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
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

#[test]
fn the_graphics_state_carries_over_between_content_streams() {
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Text at (1000, 2000) in a space scaled by 0.12: on the page at (120, 240), 12 pt.
    let mut doc = split_page("0.12 0 0 0.12 0 0 cm", "BT /F1 100 Tf 1000 2000 Td (Site plan) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let l = &lines[0];
    assert!(close(l.rect[0], 120.0) && l.rect[1] > 230.0 && l.rect[3] < 260.0, "{:?}", l.rect);
    assert!(close(l.size, 12.0), "{}", l.size);
    // Editing keeps the paragraph where it was, at its size; moving it moves it in page space.
    text::rewrite_block(&mut doc, 0, 0, Some("Site plan, rev. B"), &text::BlockStyle { offset: Some([10.0, -20.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(after[0].text, "Site plan, rev. B");
    assert!(close(after[0].rect[0], 130.0) && close(after[0].rect[1], l.rect[1] - 20.0), "{:?}", after[0].rect);
    assert!(close(after[0].size, 12.0), "{}", after[0].size);
    // A `q` left open in one stream and closed in the next still restores the state.
    let doc = split_page("q 0.5 0 0 0.5 0 0 cm", "Q BT /F1 10 Tf 100 100 Td (Footpath) Tj ET");
    let l = &text::text_lines(&doc, 0).unwrap()[0];
    assert!(close(l.rect[0], 100.0) && close(l.size, 10.0), "{l:?}");
}

/// One page whose font draws a few Hebrew letters (ToUnicode): A ב, B ד, C י, D ו, E ק, F ש,
/// G ל, H ם. Content streams draw Hebrew in visual order, so "בדיוק" is `(EDCBA)`.
fn hebrew_page(content: &str) -> Document {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 9 beginbfchar \
                <20> <0020> <41> <05D1> <42> <05D3> <43> <05D9> <44> <05D5> <45> <05E7> <46> <05E9> <47> <05DC> <48> <05DD> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    // Widths for codes 32..=72: a space, then the eight letters at 65..=72.
    let widths: Vec<&str> = (32..=72)
        .map(|c| {
            if c == 32 {
                "250"
            } else if c >= 65 {
                "500"
            } else {
                "0"
            }
        })
        .collect();
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+FakeHebrew /FirstChar 32 /LastChar 72 /Widths [{}] /ToUnicode 6 0 R >>",
            widths.join(" ")
        ),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
    ];
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

/// The report: Edit text showed "בדיוק" (exactly) reversed, because the line was read in the
/// order the page draws it.
#[test]
fn hebrew_lines_read_in_typing_order() {
    let doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET BT /F1 12 Tf 300 600 Td (EDCBA HDGF) Tj ET");
    let lines: Vec<String> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(lines, ["בדיוק", "שלום בדיוק"]);
    let blocks: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(blocks, ["בדיוק", "שלום בדיוק"]);
}

/// Text the page's own font can show is written back in drawing order, right-aligned to where
/// the line ended, and reads back as typed.
#[test]
fn hebrew_edits_are_drawn_right_to_left_and_keep_their_right_edge() {
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let before = text::text_lines(&doc, 0).unwrap()[0].rect;
    // One line: the bytes are the visual order of the typed text.
    assert_eq!(text::replace_line(&mut doc, 0, 0, "שלום בדיוק").unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(EDCBA HDGF)"), "{content}");
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "שלום בדיוק");

    // A paragraph: shorter text stays right-aligned at the old right edge.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    assert_eq!(text::replace_block(&mut doc, 0, 0, "דוב").unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(ADB)"), "{content}");
    let doc = reopen(&doc);
    let line = text::text_lines(&doc, 0).unwrap().swap_remove(0);
    assert_eq!(line.text, "דוב");
    assert!((line.rect[2] - before[2]).abs() < 0.5, "right edge {} → {}", before[2], line.rect[2]);
    assert!(line.rect[0] > before[0] + 1.0, "shorter text starts further right: {:?} vs {before:?}", line.rect);

    // Longer text grows to the left, not off the right edge.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    text::replace_block(&mut doc, 0, 0, "שלום בדיוק שלום").unwrap();
    let doc = reopen(&doc);
    let line = text::text_lines(&doc, 0).unwrap().swap_remove(0);
    assert_eq!(line.text, "שלום בדיוק שלום");
    assert!((line.rect[2] - before[2]).abs() < 0.5, "right edge {} → {}", before[2], line.rect[2]);
    assert!(line.rect[0] < before[0] - 1.0, "longer text grows left: {:?} vs {before:?}", line.rect);

    // An alignment asked for still wins.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let left = text::BlockStyle { align: Some(Align::Left), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("דוב"), &left).unwrap();
    let line = text::text_lines(&reopen(&doc), 0).unwrap().swap_remove(0);
    assert!((line.rect[0] - before[0]).abs() < 0.5, "left-aligned: {:?} vs {before:?}", line.rect);
}

/// A generated Type 3 font numbers its glyphs from 1, so code 32 is a character, not the space:
/// justified rows are stretched after each space, never with word spacing.
#[test]
fn justified_text_in_the_type3_fallback_stretches_its_spaces() {
    let src = "BT /F1 12 Tf 14 TL 100 700 Td (Some words to fill a row of text) Tj T* (and another one below it) Tj T* (end) Tj ET";
    let mut doc = text_page(src);
    let rect = text::text_blocks(&doc, 0).unwrap()[0].rect;
    // More than 32 different characters, so one of them gets code 32.
    let typed = "日本語の 文章を 書きます 東京 大阪 京都 名古屋 札幌 福岡 神戸 横浜 仙台 広島 千葉 新潟 静岡 熊本 岡山 金沢";
    let style = text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() };
    let rewritten = text::rewrite_block(&mut doc, 0, 0, Some(typed), &style);
    if pdfcraft_fonts::document_japanese_font().is_none() {
        assert!(rewritten.is_err());
        eprintln!("built without a craft-fonts Japanese face: the Type 3 path is checked with CRAFT_FONTS_DIR");
        return;
    }
    rewritten.unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.len() > 2, "{lines:?}");
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - rect[2]).abs() < 1.0, "{:?} spans {:?}, not {rect:?}", l.text, l.rect);
    }
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    assert!(ops.iter().any(|o| o.is("TJ")), "{ops:?}");
    assert!(ops.iter().filter(|o| o.is("Tw")).all(|o| o.operands.first().and_then(|n| n.as_f64()) == Some(0.0)), "{ops:?}");
}

/// Letters the page's font lacks are drawn with the Hebrew face from craft-fonts; a build
/// without it says so and changes nothing. Hebrew points are refused, not misdrawn.
#[test]
fn hebrew_letters_the_page_font_lacks_use_the_hebrew_face() {
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let pointed = text::replace_line(&mut doc, 0, 0, "שָׁלוֹם עוֹלָם");
    let Err(EditError::Invalid(msg)) = pointed else { panic!("expected a clear error, got {pointed:?}") };
    assert!(msg.contains("niqqud"), "{msg}");
    assert_eq!(page_content_bytes(&doc, 0), before);

    // ע and ה aren't in the page's font.
    let typed = "שלום עולם, עמוד 12 (PDF)";
    let line = text::replace_line(&mut doc, 0, 0, typed);
    if pdfcraft_fonts::document_hebrew_font(false).is_none() {
        let Err(EditError::Invalid(msg)) = line else { panic!("expected a clear error, got {line:?}") };
        assert!(msg.contains("Hebrew fallback font") && msg.contains("CRAFT_FONTS_DIR"), "{msg}");
        assert_eq!(page_content_bytes(&doc, 0), before);
        eprintln!("built without a craft-fonts Hebrew face: the Type 3 path is checked with CRAFT_FONTS_DIR");
        return;
    }
    assert_eq!(line.unwrap().substituted.as_deref(), Some("Noto Sans Hebrew Type3"));
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("/PCHe"), "{content}");
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, typed);
    // Every glyph has an outline, so nothing is drawn as a box.
    let p = pdfcraft_model::pages(&reopened).swap_remove(0);
    let res = reopened.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = reopened.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = reopened.resolve(fonts.as_dict().unwrap().get(b"PCHe").unwrap());
    let procs = reopened.resolve(font.as_dict().unwrap().get(b"CharProcs").unwrap());
    let procs = procs.as_dict().unwrap();
    assert_eq!(procs.iter().count(), typed.chars().collect::<std::collections::HashSet<_>>().len());
    for (_, glyph) in procs.iter() {
        let pdfcraft_cos::Object::Stream(s) = &*reopened.resolve(glyph) else { panic!("glyph stream") };
        let path = String::from_utf8(s.decoded().unwrap()).unwrap();
        assert!(path.starts_with(|c: char| c.is_ascii_digit()) && path.contains(" d1"), "{path}");
    }

    // A whole paragraph too, right-aligned.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    assert_eq!(text::replace_block(&mut doc, 0, 0, "עברית מלאה").unwrap().substituted.as_deref(), Some("Noto Sans Hebrew Type3"));
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "עברית מלאה");
}

/// #2013: the dialog asks `first_undrawable` before offering OK, so it must refuse exactly what
/// applying refuses: a Cyrillic footer was previewed (in the interface font) and then failed.
#[test]
fn header_and_watermark_drawability_matches_what_applying_refuses() {
    let footer = |t: &str| HeaderFooter {
        text: [String::new(), String::new(), String::new(), String::new(), String::new(), t.into()],
        ..HeaderFooter::default()
    };
    for (text, refused) in [
        ("Страница <<1>> из 1 / Page <<1>> of 1", Some('С')),
        ("TEST-001 <<Page 1 of n>>", None),
        ("Café — 5€ ™ <<Bates Number#6#1#ACME-#>>", None),
        ("Left\r\nRight", None),
        ("A\rB", Some('\r')),
        ("line one\nΕλληνικά", Some('Ε')),
    ] {
        let hf = footer(text);
        assert_eq!(hf.first_undrawable(), refused, "{text:?}");
        let mut doc = fixture();
        let applied = add_header_footer(&mut doc, &[0], &hf, false, &cx());
        match refused {
            Some(c) => assert_eq!(applied.unwrap_err().to_string(), undrawable_message(c), "{text:?}"),
            None => applied.unwrap(),
        }
        let wm = Watermark { text: text.into(), ..Watermark::default() };
        assert_eq!(wm.first_undrawable(), refused, "{text:?}");
        let applied = add_watermark(&mut fixture(), &[0], &wm, false);
        assert_eq!(applied.err().map(|e| e.to_string()), refused.map(undrawable_message), "{text:?}");
    }
}

/// #125: page-content tools refuse text the standard fonts can't draw instead of writing `?`,
/// and leave the document untouched.
#[test]
fn page_text_the_standard_fonts_cant_draw_is_refused() {
    let mut doc = fixture();
    let before: Vec<Vec<String>> = (0..3).map(|p| streams(&reopen(&doc), p)).collect();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "日本語のテキスト".into(), ..AddedText::default() };
    let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err().to_string();
    assert!(err.contains("\"日\" (U+65E5)"), "{err}");
    let hf = HeaderFooter {
        text: ["Ελληνικά".into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    assert!(add_header_footer(&mut doc, &[0, 1], &hf, true, &cx()).unwrap_err().to_string().contains("U+0395"));
    let wm = Watermark { text: "机密".into(), ..Watermark::default() };
    assert!(add_watermark(&mut doc, &[0], &wm, true).unwrap_err().to_string().contains("U+673A"));
    let after: Vec<Vec<String>> = (0..3).map(|p| streams(&reopen(&doc), p)).collect();
    assert_eq!(before, after, "a refused edit changes nothing");
    // Western European text, including what only WinAnsi (not Latin-1) has, still works.
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Café — 5€ ™".into(), ..AddedText::default() };
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // Retyping existing text is refused the same way, and the whole document is left as it was.
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let retyped = AddedText { text: "Café 中".into(), ..text.clone() };
    assert!(update_content(&mut doc, 0, 0, &Content::Text(retyped)).unwrap_err().to_string().contains("U+4E2D"));
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved, "a refused update changes nothing");
    // A carriage return is checked where it would be drawn: added text splits on LF only, so
    // CRLF would leave a `?`; headers and watermarks split CRLF into lines, but not a lone CR.
    let crlf = AddedText { text: "A\r\nB".into(), ..text };
    assert!(add_content(&mut doc, 0, &Content::Text(crlf)).unwrap_err().to_string().contains("U+000D"));
    let hf = |t: &str| HeaderFooter {
        text: [t.into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    add_header_footer(&mut doc, &[0], &hf("Left\r\n<<Bates Number#6#1#ACME-#>>"), true, &cx()).unwrap();
    assert!(add_header_footer(&mut doc, &[0], &hf("A\rB"), true, &cx()).unwrap_err().to_string().contains("U+000D"));
    // Arabic is shaped with the craft-fonts Arabic face (#403): drawable when that face is built
    // in, refused (never written as `?`) when it isn't. Text left to the standard font still counts.
    let arabic = |text: &str| AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: text.into(), ..AddedText::default() };
    // U+061C is in the Arabic block but is a direction mark that's dropped, not shaped: it never
    // makes text drawable, nor (with the face) undrawable.
    assert!(add_content(&mut doc, 0, &Content::Text(arabic("\u{061C}中"))).is_err());
    if pdfcraft_fonts::arabic_has('م') {
        add_content(&mut doc, 0, &Content::Text(arabic("مرحبا Hello"))).unwrap();
        add_content(&mut doc, 0, &Content::Text(arabic("\u{061C}Hello"))).unwrap();
        for text in ["مرحبا 中", "\u{061C}中"] {
            let err = add_content(&mut doc, 0, &Content::Text(arabic(text))).unwrap_err().to_string();
            assert!(err.contains("U+4E2D"), "{text:?}: {err}");
        }
    } else {
        // #403's own refusal says what's missing; the Add-text editor still keeps the draft.
        let err = add_content(&mut doc, 0, &Content::Text(arabic("مرحبا Hello"))).unwrap_err().to_string();
        assert!(err.contains("CRAFT_FONTS_DIR"), "{err}");
        assert_eq!(first_undrawable(&arabic("مرحبا Hello")), Some('م'));
        // An existing item may still take Arabic (#403 keeps it movable), but nothing else that
        // the standard font would write as `?`.
        let n = list_added(&doc).iter().filter(|a| a.page == 0).count();
        add_content(&mut doc, 0, &Content::Text(arabic("x"))).unwrap();
        let err = update_content(&mut doc, 0, n, &Content::Text(arabic("ب中"))).unwrap_err().to_string();
        assert!(err.contains("U+4E2D"), "{err}");
        update_content(&mut doc, 0, n, &Content::Text(arabic("ب"))).unwrap();
    }
}

fn xobject(doc: &Document, page: usize, name: &[u8]) -> Option<pdfcraft_cos::ObjRef> {
    let p = &pdfcraft_model::pages(doc)[page];
    let res = doc.resolve(p.dict.get(b"Resources")?);
    let xo = doc.resolve(res.as_dict()?.get(b"XObject")?);
    xo.as_dict()?.get(name).and_then(Object::as_ref)
}

fn annot_at(doc: &Document, page: usize, index: usize) -> pdfcraft_cos::ObjRef {
    let p = &pdfcraft_model::pages(doc)[page];
    let list = doc.resolve(p.dict.get(b"Annots").unwrap());
    list.as_array().unwrap()[index].as_ref().unwrap()
}

#[test]
fn fill_sign_flatten_bakes_marks_keeps_other_comments_and_does_not_reuse_pcfl0() {
    use pdfcraft_annot::{FillMark, Markup, Meta, NewAnnotation, Shape, Style, add_annotation, summaries};
    let mut doc = fixture();
    assert_eq!(flatten_fill_sign(&mut doc, &[0]).unwrap(), 0);
    assert!(!doc.is_modified(), "nothing to flatten leaves the file alone");

    let meta = Meta { date: None, id: "x".into() };
    let add = |doc: &mut Document, shape: Shape, contents: &str| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: contents.into(), author: "a".into() }, &meta).unwrap()
    };
    add(&mut doc, Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] }, "box");
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1);
    let kept = xobject(&doc, 0, b"PCFl0").expect("the first flatten's XObject");

    add(&mut doc, Shape::Typewriter { rect: [20.0, 500.0, 140.0, 520.0], font_size: 10.0 }, "Hello");
    doc.update_dict(annot_at(&doc, 0, 0), |d| {
        d.remove(b"PCFillSign");
    })
    .unwrap();
    add(&mut doc, Shape::Mark { rect: [100.0, 100.0, 120.0, 120.0], mark: FillMark::Check }, "");
    add(&mut doc, Shape::Signature { strokes: vec![vec![[10.0, 50.0], [40.0, 80.0], [70.0, 50.0]]] }, "");
    doc.update_dict(annot_at(&doc, 0, 2), |d| d.set(b"Subj".to_vec(), pdfcraft_cos::PdfString::text("Pencil"))).unwrap();
    add(&mut doc, Shape::TypedSignature { rect: [20.0, 40.0, 90.0, 70.0], contours: vec![vec![[0.1, 0.2], [0.5, 0.9], [0.9, 0.2]]] }, "");
    add(&mut doc, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 400.0, 80.0, 400.0, 20.0, 390.0, 80.0, 390.0]] }, "keep");
    add(&mut doc, Shape::TextBox { rect: [20.0, 300.0, 140.0, 340.0], font_size: 12.0 }, "Note");
    add(&mut doc, Shape::Ink { strokes: vec![vec![[200.0, 200.0], [220.0, 220.0], [240.0, 200.0]]] }, "pencil");
    let old = add(&mut doc, Shape::Signature { strokes: vec![vec![[300.0, 50.0], [330.0, 80.0], [360.0, 50.0]]] }, "");
    doc.update_dict(annot_at(&doc, 0, old), |d| {
        d.remove(b"PCFillSign");
    })
    .unwrap();
    let hidden = add(&mut doc, Shape::Typewriter { rect: [20.0, 600.0, 140.0, 620.0], font_size: 10.0 }, "secret");
    doc.update_dict(annot_at(&doc, 0, hidden), |d| d.set(b"F".to_vec(), Object::Int(2))).unwrap();

    let n = flatten_fill_sign(&mut doc, &[0]).unwrap();
    assert_eq!(n, 5, "text, check, both signatures and the typed signature; the hidden one stays");
    assert_eq!(xobject(&doc, 0, b"PCFl0"), Some(kept), "an existing PCFl0 is not replaced");
    let newest = streams(&doc, 0).last().unwrap().clone();
    assert!(newest.contains("/PCFl1 Do") && !newest.contains("/PCFl0 Do"), "{newest}");

    let doc = reopen(&doc);
    let mut left: Vec<String> = summaries(&doc).iter().map(|s| s.contents.clone().unwrap_or_default()).collect();
    left.sort();
    assert_eq!(left, ["Note", "keep", "pencil", "secret"]);
    assert!(summaries(&doc).iter().any(|s| s.intent.as_deref() == Some("FreeTextTypeWriter")), "the hidden typewriter stays");
    assert_eq!(summaries(&doc).iter().filter(|s| s.intent.as_deref() == Some("FreeTextTypeWriter")).count(), 1);
}

/// A one-page document from its objects (object 1 is the catalog).
fn build(objs: &[&str]) -> Document {
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

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// #314: text and images drawn by (nested) form XObjects are read for export, placed through
/// every form matrix and set in each form's own fonts; editing still sees only the page's text.
#[test]
fn reading_follows_form_xobjects() {
    let page = stream("", "q 1 0 0 1 10 0 cm /Fm1 Do Q BT /F1 12 Tf 72 100 Td (Page text) Tj ET");
    let fm1 = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Matrix [1 0 0 1 0 50] /Resources << /Font << /F1 7 0 R >> /XObject << /Fm2 8 0 R >> >>",
        "BT /F1 18 Tf 72 700 Td (Hello from a test invoice) Tj ET /Fm2 Do",
    );
    let fm2 = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Matrix [2 0 0 2 0 0] /Resources << /Font << /F2 9 0 R >> /XObject << /Im1 10 0 R >> >>",
        "BT /F2 10 Tf 50 100 Td (Nested) Tj ET q 20 0 0 10 5 5 cm /Im1 Do Q",
    );
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "A");
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        &fm1,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &fm2,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >>",
        &image,
    ]);
    let close = |a: f64, b: f64| (a - b).abs() < 0.01;

    let blocks = text::reading_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Hello from a test invoice", "Nested", "Page text"]);
    let (hello, nested, own) = (&blocks[0], &blocks[1], &blocks[2]);
    // Hello: (72, 700) moved by the form's (0, 50) and the page's (10, 0).
    assert_eq!(hello.base_font, "Helvetica", "the form's own /F1, not the page's");
    assert!(close(hello.rect[0], 82.0) && close(hello.size, 18.0), "{hello:?}");
    // Nested: (50, 100) doubled by Fm2, then Fm1 and the page: (110, 250), 20 pt.
    assert_eq!(nested.base_font, "Times-Bold");
    assert!(nested.bold);
    assert!(close(nested.rect[0], 110.0) && close(nested.size, 20.0), "{nested:?}");
    assert_eq!(own.base_font, "Courier");

    let images = images::reading_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1, "{images:?}");
    let r = images[0].rect;
    assert!(close(r[0], 20.0) && close(r[1], 60.0) && close(r[2], 60.0) && close(r[3], 80.0), "{r:?}");
    assert_eq!((images[0].width, images[0].height), (1, 1));

    // Editing works on the page's own streams only, unchanged.
    let editable: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(editable, ["Page text"]);
    // The page draws one figure of its own (Fm1, edited as a whole); the image inside Fm2 is not
    // a page image.
    let own = images::page_images(&doc, 0).unwrap();
    assert_eq!(own.len(), 1, "{own:?}");
    assert!(own[0].is_form && own[0].object == Some(pdfcraft_cos::ObjRef::new(6, 0)), "{own:?}");
}

/// A chain of forms, each drawing the next twenty times, twelve deep: within the depth cap but
/// 20^11 visits without a budget. Reading stops at the page's visit budget instead of hanging.
#[test]
fn form_xobjects_that_fan_out_stop_at_the_visit_budget() {
    let font = "/Font << /F1 5 0 R >>";
    let levels = 12;
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /N 6 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        stream("", "/N Do"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for level in 0..levels {
        let next = 7 + level;
        objs.push(if level + 1 == levels {
            stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} >>"), "BT /F1 10 Tf 10 10 Td (leaf) Tj ET")
        } else {
            stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /N {next} 0 R >> >>"), &"/N Do ".repeat(20))
        });
    }
    let refs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let doc = build(&refs);
    let start = std::time::Instant::now();
    let _ = text::reading_blocks(&doc, 0).unwrap();
    assert!(images::reading_images(&doc, 0).unwrap().is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(30), "took {:?}", start.elapsed());
}

/// #3120: a one-page document whose page draws "Secret", then `content`. `forms` are (name,
/// content) form XObjects, which the page and every form can draw, as they can a 1×1 image `/Im`.
fn page_with_forms(content: &str, forms: &[(String, String)]) -> Document {
    let xobjects: String = forms.iter().enumerate().map(|(i, (name, _))| format!("/{name} {} 0 R ", 7 + i)).collect();
    let resources = format!("<< /Font << /F1 5 0 R >> /XObject << /Im 6 0 R {xobjects}>> >>");
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} /Contents 4 0 R >>"),
        stream("", &format!("BT /F1 24 Tf 72 700 Td (Secret) Tj ET\n{content}")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "A"),
    ];
    objs.extend(forms.iter().map(|(_, body)| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources {resources}"), body)));
    build(&objs.iter().map(String::as_str).collect::<Vec<_>>())
}

/// #3120: export reads text from forms as deep and as often as the renderer draws them, and says
/// when a limit left some out: a text form drawn 5 000 times (past the old budget of 4 096) or
/// forms nested 13 deep (past the old 12) lost the form drawn after them, silently.
#[test]
fn reading_follows_forms_as_far_as_the_renderer_and_says_where_it_stops() {
    // Covers "Secret" with a black box and writes "TAIL": drawn last, so a reader that stops
    // early misses it.
    const TAIL: &str = "0 g 70 692 90 30 re f BT /F1 24 Tf 72 600 Td (TAIL) Tj ET";
    let tail = |doc: &Document| {
        let read = text::reading_blocks_checked(doc, 0).unwrap();
        (read.blocks.iter().any(|b| b.text.contains("TAIL")), read.truncated)
    };
    let form = |name: &str, body: &str| (name.to_string(), body.to_string());

    let rows =
        |n: usize| page_with_forms(&format!("{}/C Do", "/R Do ".repeat(n)), &[form("R", "BT /F1 6 Tf 300 100 Td (row) Tj ET"), form("C", TAIL)]);
    assert_eq!(tail(&rows(5_000)), (true, false));

    // The innermost form also draws an image: an image at the depth limit is read, and leaves
    // nothing out.
    let chain = |depth: usize| {
        let mut forms: Vec<_> = (1..depth).map(|i| form(&format!("D{i}"), &format!("/D{} Do", i + 1))).collect();
        forms.push(form(&format!("D{depth}"), &format!("{TAIL} q 20 0 0 10 5 5 cm /Im Do Q")));
        page_with_forms("/D1 Do", &forms)
    };
    assert_eq!(tail(&chain(13)), (true, false));
    // The renderer's limit, written out so the test does not move with the constant.
    assert_eq!(tail(&chain(50)), (true, false));
    assert_eq!(tail(&chain(51)), (false, true));
    let deepest = chain(text::MAX_FORM_DEPTH);
    assert_eq!(tail(&deepest), (true, false));
    assert_eq!(images::reading_images(&deepest, 0).unwrap().len(), 1);
    let deep = chain(text::MAX_FORM_DEPTH + 1);
    assert_eq!(tail(&deep), (false, true));
    assert!(images::reading_images(&deep, 0).unwrap().is_empty());

    // A wrapper form A draws a dot form B `n` times, then C. A, the dots and C use the page's
    // budget up exactly; one dot more leaves C unread.
    let fanout =
        |n: usize| page_with_forms("/A Do", &[form("A", &format!("{}/C Do", "/B Do ".repeat(n))), form("B", "0 g 10 10 1 1 re f"), form("C", TAIL)]);
    assert_eq!(tail(&fanout(text::MAX_FORM_VISITS - 2)), (true, false));
    let many = fanout(text::MAX_FORM_VISITS - 1);
    assert_eq!(tail(&many), (false, true));
    assert!(images::reading_images(&many, 0).unwrap().is_empty());
    // Without the flag, reading is unchanged.
    let blocks: Vec<String> = text::reading_blocks(&many, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(blocks, ["Secret"]);
}

/// #3120: with the renderer's limits, forms that each draw the next a hundred times, 12 levels
/// deep (100^11 forms) or past the depth cap, still stop at the page's budget quickly, and say
/// that text was left out.
#[test]
fn forms_that_fan_out_a_hundred_ways_stop_quickly_and_say_so() {
    for levels in [12, text::MAX_FORM_DEPTH + 1] {
        let mut forms: Vec<(String, String)> = (1..levels).map(|i| (format!("N{i}"), format!("/N{} Do ", i + 1).repeat(100))).collect();
        forms.push((format!("N{levels}"), "BT /F1 10 Tf 10 10 Td (leaf) Tj ET".to_string()));
        let doc = page_with_forms("/N1 Do", &forms);
        let start = std::time::Instant::now();
        assert!(text::reading_blocks_checked(&doc, 0).unwrap().truncated, "{levels} levels");
        assert!(images::reading_images(&doc, 0).unwrap().is_empty());
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "{levels} levels took {:?}", start.elapsed());
    }
}

/// Forms that draw themselves or each other, have no resources, a malformed matrix, or are
/// missing are read once (or skipped) without panicking or looping.
#[test]
fn hostile_form_xobjects_are_read_once() {
    let page = stream("", "/Loop Do /A Do /NoRes Do /Bad Do /Missing Do /F1 Do");
    let font = "/Font << /F1 5 0 R >>";
    let lp = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /Loop 6 0 R >> >>"),
        "/Loop Do BT /F1 10 Tf 10 10 Td (once) Tj ET",
    );
    let a = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /B 8 0 R >> >>"),
        "/B Do BT /F1 10 Tf 10 100 Td (from a) Tj ET",
    );
    let b = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /A 7 0 R >> >>"),
        "/A Do BT /F1 10 Tf 10 200 Td (from b) Tj ET",
    );
    let nores = stream("/Subtype /Form /BBox [0 0 9 9]", "BT /F1 10 Tf 10 300 Td (inherited) Tj ET");
    let bad = stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Matrix [1 0 0] /Resources << {font} >>"), "BT /F1 10 Tf 10 400 Td (bad matrix) Tj ET");
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Loop 6 0 R /A 7 0 R /NoRes 9 0 R /Bad 10 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &lp,
        &a,
        &b,
        &nores,
        &bad,
    ]);
    let texts: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["once", "from b", "from a", "inherited", "bad matrix"]);
    assert!(images::reading_images(&doc, 0).unwrap().is_empty());
}

#[test]
fn added_thai_text_wraps_clusters_and_survives_reopen() {
    let mut doc = fixture();
    let t = AddedText {
        rect: [20.0, 100.0, 250.0, 300.0], text: "สวัสดี น้ำ ผู้ใช้ ปู่ ๑๒๓ Hello 123".into(), size: 16.0, ..AddedText::default()
    };
    if pdfcraft_fonts::document_thai_font().is_none() {
        assert_eq!(first_undrawable(&t), Some('ส'));
        let before = list_added(&doc);
        let err = add_content(&mut doc, 0, &Content::Text(t)).unwrap_err().to_string();
        assert!(err.contains("Thai") && err.contains("CRAFT_FONTS_DIR"), "{err}");
        assert_eq!(list_added(&doc), before);
        return;
    }
    assert_eq!(first_undrawable(&t), None);
    add_content(&mut doc, 0, &Content::Text(t.clone())).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(list_added(&reopened)[0].content, Content::Text(AddedText { rect: super::added::text_rect(&t), ..t.clone() }));
    let narrow = AddedText { rect: [0.0, 0.0, 16.0, 200.0], text: "กิ่กิ่กิ่".into(), ..t.clone() };
    assert_eq!(super::added::lines(&narrow), ["กิ่", "กิ่", "กิ่"]);
    update_content(&mut doc, 0, 0, &Content::Text(AddedText { text: "แก้ไข น้ำ".into(), ..t.clone() })).unwrap();
    assert!(!own(&doc, 0).is_empty());
    let bad = Content::Text(AddedText { text: "ไทย 日".into(), ..t });
    let before = list_added(&doc);
    assert!(update_content(&mut doc, 0, 0, &bad).is_err());
    assert_eq!(list_added(&doc), before);
    delete_content(&mut doc, 0, 0).unwrap();
    assert!(list_added(&doc).is_empty());
    assert!(arabic_font_names(&doc, 0).is_empty());
}

#[test]
fn group_moves_preserve_glyph_bytes_and_unselected_text_state() {
    for content in [
        "BT /F1 12 Tf 1.3 Tc 2 Tw 80 Tz 14 TL 1 0.2 -0.1 1 50 600 Tm (A\\101) Tj 1 0 0 rg (Neighbour) Tj T* (Next) Tj ET",
        "BT /F1 12 Tf 14 TL 50 600 Td (Before) Tj 0 0 1 rg (Quoted) ' 1 0 0 rg (After) Tj ET",
        "BT /F1 12 Tf 14 TL 50 600 Td (Before) Tj 0 0 1 rg 2 1 (Quoted) \" 1 0 0 rg [(After) -120 (wards)] TJ T* (Last) Tj ET",
        "q 1.2 0.3 -0.1 0.9 20 30 cm BT /F1 12 Tf 50 600 Td [(A) -120 (B)] TJ 1 0 0 rg (After) Tj ET Q %keep-this-comment\n",
        "BT /F1 12.3456789 Tf 81.23456789 Tz 0.123456789 Ts 14 TL 50 600 Td (Before) Tj 0 0 1 rg 2.123456789 1.987654321 (Quoted) \" 1 0 0 rg (After) Tj ET",
    ] {
        let mut doc = text_page(content);
        let before = text::text_lines(&doc, 0).unwrap();
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let index = blocks.iter().position(|b| b.text.contains("Quoted")).unwrap_or(0);
        let selected: Vec<_> = blocks[index].lines.iter().map(|i| before[*i].text.clone()).collect();
        let glyph_bytes = |doc: &Document| {
            pdfcraft_content::parse(&page_content_bytes(doc, 0))
                .ops
                .iter()
                .flat_map(|op| {
                    op.operands
                        .iter()
                        .flat_map(|o| match o {
                            Object::String(s) => vec![s.bytes.clone()],
                            Object::Array(a) => a.iter().filter_map(|v| v.as_string().map(|s| s.bytes.clone())).collect(),
                            _ => Vec::new(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let original_bytes = glyph_bytes(&doc);
        let target = ObjectTarget { kind: ObjectKind::Text, index };
        move_objects(&mut doc, 0, &[target], [35.0, -27.0]).unwrap();
        let doc = reopen(&doc);
        assert_eq!(glyph_bytes(&doc), original_bytes, "original codes and kerning strings survive");
        let after = text::text_lines(&doc, 0).unwrap();
        assert_eq!(after.len(), before.len(), "{content}");
        for old in before {
            let new = after.iter().find(|l| l.text == old.text).unwrap();
            let expected = if selected.contains(&old.text) {
                [old.rect[0] + 35.0, old.rect[1] - 27.0, old.rect[2] + 35.0, old.rect[3] - 27.0]
            } else {
                old.rect
            };
            assert!(new.rect.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6), "{}: {:?} != {:?}", old.text, new.rect, expected);
            assert_eq!((new.font.clone(), new.color), (old.font, old.color));
        }
        if content.contains("%keep-this-comment") {
            assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("%keep-this-comment"));
        }
    }
}

#[test]
fn group_moves_preserve_small_offsets_under_large_transforms() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let content = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 100000 0 0 100000 100 100 cm /Im0 Do Q q 10000 0 0 10000 0 0 cm BT /F1 0.00123456789 Tf 0.02 0.06 Td (Small) Tj 1 0 0 rg (Neighbour) Tj ET Q",
    )));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    let before = editable_objects(&doc, 0).unwrap();
    let targets = before.iter().filter(|o| o.text.as_deref() != Some("Neighbour")).map(|o| o.target).collect::<Vec<_>>();
    assert_eq!(targets.len(), 2);
    let image_before = images::page_images(&doc, 0).unwrap()[0].rect;
    let offset = [0.00123456789, -0.00987654321];
    move_objects(&mut doc, 0, &targets, offset).unwrap();
    let reopened = reopen(&doc);
    let image_after = images::page_images(&reopened, 0).unwrap()[0].clone();
    assert!(
        close(image_after.rect, [image_before[0] + offset[0], image_before[1] + offset[1], image_before[2] + offset[0], image_before[3] + offset[1]]),
        "full image placement preserves sub-point movement"
    );
    let after = editable_objects(&reopened, 0).unwrap();
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let [dx, dy] = if targets.contains(&old.target) { offset } else { [0.0, 0.0] };
        if old.target.kind == crate::ObjectKind::Image {
            // The full placement moves; the visible right/top edges stay at the page crop.
            assert_eq!(Some(new.rect), image_after.visible_rect);
        } else {
            assert!(close(new.rect, [old.rect[0] + dx, old.rect[1] + dy, old.rect[2] + dx, old.rect[3] + dy]), "{old:?}: {new:?}");
        }
    }
}

#[test]
fn mixed_object_inventory_does_not_select_added_content_twice() {
    let mut doc = image_page();
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "Added label".into(), rect: [50.0, 350.0, 200.0, 380.0], ..AddedText::default() }))
        .unwrap();
    let before = editable_objects(&doc, 0).unwrap();
    assert_eq!(before.len(), 3, "one native paragraph, one native image, one added text");
    let targets = before.iter().map(|o| o.target).collect::<Vec<_>>();
    let image = images::page_images(&doc, 0).unwrap()[0].object.unwrap();
    let original_image = doc.get(image).clone();
    move_objects(&mut doc, 0, &targets, [20.0, -15.0]).unwrap();
    let doc = reopen(&doc);
    let after = editable_objects(&doc, 0).unwrap();
    assert_eq!(after.len(), before.len());
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        assert!(close(new.rect, [old.rect[0] + 20.0, old.rect[1] - 15.0, old.rect[2] + 20.0, old.rect[3] - 15.0]));
        assert_eq!(new.text, old.text);
    }
    assert_eq!(doc.get(image).clone(), original_image, "pixel data is shared, not recompressed");
}

#[test]
fn group_move_refusals_leave_the_object_graph_untouched() {
    let mut doc = image_page();
    let target = editable_objects(&doc, 0).unwrap()[0].target;
    let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    for (targets, offset) in [
        (vec![], [1.0, 2.0]),
        (vec![target, target], [1.0, 2.0]),
        (vec![target, ObjectTarget { kind: ObjectKind::Image, index: usize::MAX }], [1.0, 2.0]),
        (vec![target], [f64::NAN, 2.0]),
        (vec![target], [f64::INFINITY, 2.0]),
        (vec![target], [0.0, 0.0]),
        (vec![target], [1e300, 2.0]),
        (vec![target; MAX_MOVE_OBJECTS + 1], [1.0, 2.0]),
    ] {
        assert!(move_objects(&mut doc, 0, &targets, offset).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
    }
    let mut clipped = text_page("BT /F1 12 Tf 50 600 Td (Ordinary) Tj ET BT /F1 12 Tf 4 Tr 50 400 Td (Clipping) Tj ET");
    let targets = editable_objects(&clipped, 0).unwrap().iter().map(|o| o.target).collect::<Vec<_>>();
    let original = pdfcraft_cos::write_full(&clipped, &SaveOptions::default()).unwrap();
    assert!(move_objects(&mut clipped, 0, &targets, [10.0, 5.0]).unwrap_err().to_string().contains("clipping"));
    assert_eq!(pdfcraft_cos::write_full(&clipped, &SaveOptions::default()).unwrap(), original);
}

#[test]
fn added_and_native_objects_move_together_on_cropped_rotated_user_unit_pages() {
    for rotation in [0, 90, 180, 270] {
        let mut doc = image_page();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| {
            d.set(b"CropBox".to_vec(), Object::Array([10, 20, 500, 750].map(Object::Int).to_vec()));
            d.set(b"Rotate".to_vec(), Object::Int(rotation));
            d.set(b"UserUnit".to_vec(), Object::Int(2));
        })
        .unwrap();
        add_content(&mut doc, 0, &Content::Text(AddedText { text: "Added".into(), rect: [60.0, 100.0, 180.0, 125.0], ..AddedText::default() }))
            .unwrap();
        let before = editable_objects(&doc, 0).unwrap();
        move_objects(&mut doc, 0, &before.iter().map(|o| o.target).collect::<Vec<_>>(), [12.0, -8.0]).unwrap();
        let after = editable_objects(&reopen(&doc), 0).unwrap();
        for (a, b) in before.iter().zip(after) {
            assert!(
                close(b.rect, [a.rect[0] + 12.0, a.rect[1] - 8.0, a.rect[2] + 12.0, a.rect[3] - 8.0]),
                "rotation {rotation}: {:?} -> {:?}",
                a.rect,
                b.rect
            );
        }
    }
}

#[test]
fn group_moves_keep_split_stream_tokens_and_shared_form_resources() {
    let mut doc = split_streams_page();
    let before = text::text_lines(&doc, 0).unwrap();
    let objects: Vec<_> = editable_objects(&doc, 0).unwrap().iter().map(|o| o.target).collect();
    move_objects(&mut doc, 0, &objects, [30.0, -20.0]).unwrap();
    let doc = reopen(&doc);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.text, b.text);
        assert!(close(b.rect, [a.rect[0] + 30.0, a.rect[1] - 20.0, a.rect[2] + 30.0, a.rect[3] - 20.0]));
    }
    let joined = page_content_bytes(&doc, 0);
    let ops = pdfcraft_content::parse(&joined).ops;
    assert_eq!(ops.iter().find(|o| o.is("BDC")).unwrap().operands.len(), 2);
    assert!(ops.iter().any(|o| o.is("EMC")));
    // Two pages share a content stream and a Form; only one placement on one page moves.
    let mut doc = fixture();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 60, 30].map(Object::Int).to_vec()));
    form.set(b"PrivateData".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"retain this".to_vec())));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"1 0 0 rg 0 0 60 30 re f")));
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |d| {
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        d.set(b"XObject".to_vec(), Object::Dict(xo));
    })
    .unwrap();
    doc.set(
        pdfcraft_cos::ObjRef::new(7, 0),
        Object::Stream(Stream::flate(
            Dict::new(),
            b"q 1 0 0 1 40 80 cm /Figure Do Q q 1 0 0 1 300 400 cm /Figure Do Q BT /F1 9 Tf 40 160 Td (Caption) Tj ET",
        )),
    );
    let original_form = doc.get(artwork);
    let source_page = page_content_bytes(&doc, 1);
    let images = images::page_images(&doc, 0).unwrap();
    move_objects(
        &mut doc,
        0,
        &[ObjectTarget { kind: ObjectKind::Image, index: 0 }, ObjectTarget { kind: ObjectKind::Text, index: 0 }],
        [15.0, -10.0],
    )
    .unwrap();
    let after = images::page_images(&doc, 0).unwrap();
    assert!(close(after[0].rect, [55.0, 70.0, 115.0, 100.0]));
    assert_eq!(after[1].rect, images[1].rect);
    assert_eq!(*doc.get(artwork), *original_form);
    assert_eq!(page_content_bytes(&doc, 1), source_page);
}

#[test]
fn justified_text_in_a_composite_font_fills_each_row_with_adjustments_after_its_spaces() {
    // Two-byte codes, so word spacing (single-byte code 32 only) can't stretch the spaces.
    let hex = |s: &str| s.bytes().map(|b| format!("00{b:02x}")).collect::<String>();
    let src = format!(
        "BT /F1 10 Tf 12 TL 100 700 Td <{}> Tj T* <{}> Tj T* <{}> Tj ET",
        hex("One two three four five six seven"),
        hex("eight nine ten eleven twelve"),
        hex("and then the end of it")
    );
    let mut doc = text_page(&src);
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfrange <0020> <007e> <0020> endbfrange",
    )));
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("SyntheticCID")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
        (
            b"DescendantFonts".to_vec(),
            Object::Array(vec![Object::Dict(Dict::from_iter([
                (b"Subtype".to_vec(), Object::name("CIDFontType2")),
                (b"DW".to_vec(), Object::Int(500)),
            ]))]),
        ),
    ]);
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let resources = Dict::from_iter([(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Dict(font))])))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let right = before.rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.len() > 2, "{lines:?}");
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
    // The source font shows the text, with its spaces, and the text reads as before, rewrapped.
    let flat = |s: &str| s.replace('\n', " ");
    assert_eq!(flat(&text::text_blocks(&doc, 0).unwrap()[0].editor_text), flat(&before.editor_text));
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    assert!(ops.iter().all(|o| !o.is("Tf") || o.operands.first().and_then(|n| n.as_name()) == Some(b"F1".as_slice())), "{ops:?}");
    assert!(ops.iter().any(|o| o.is("TJ")), "{ops:?}");
    assert!(ops.iter().filter(|o| o.is("Tw")).all(|o| o.operands.first().and_then(|n| n.as_f64()) == Some(0.0)), "{ops:?}");
}

#[test]
fn group_moves_preserve_encoded_glyphs_and_refuse_vertical_cmaps() {
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td <004100420043> Tj ET");
    let mut font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("SyntheticCID")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
    ]);
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <ffff> endcodespacerange 3 beginbfchar <0041> <0041> <0042> <0042> <0043> <0043> endbfchar",
    )));
    font.set(b"ToUnicode".to_vec(), Object::Ref(unicode));
    font.set(
        b"DescendantFonts".to_vec(),
        Object::Array(vec![Object::Dict(Dict::from_iter([(b"Subtype".to_vec(), Object::name("CIDFontType2")), (b"DW".to_vec(), Object::Int(600))]))]),
    );
    // No external font asset: this synthetic font is enough to exercise raw CID bytes.
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let mut resources = Dict::new();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Dict(font.clone()));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
    let original = text::text_lines(&doc, 0).unwrap();
    assert!(!original.is_empty());
    move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [25.0, 15.0]).unwrap();
    let encoded =
        |doc: &Document| pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops.into_iter().find(|o| o.is("Tj")).unwrap().operands[0].clone();
    assert_eq!(encoded(&doc).as_string().unwrap().bytes, [0, 65, 0, 66, 0, 67]);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(original[0].text, after[0].text);
    for encoding in [
        Object::name("Identity-V"),
        Object::Stream(Stream::flate(Dict::new(), b"/WMode 1 def")),
        Object::Stream(Stream::flate(Dict::from_iter([(b"WMode".to_vec(), Object::Int(1))]), b"")),
    ] {
        let mut f = font.clone();
        f.set(b"Encoding".to_vec(), encoding);
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Dict(f));
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [5.0, 5.0]).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn moving_added_content_after_page_rotation_preserves_streams_and_resources() {
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td (Native) Tj ET");
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Added label".into(), rect: [50.0, 300.0, 200.0, 330.0], ..Default::default() }),
    )
    .unwrap();
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let resources = pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").cloned();
    let item = added::list_added(&doc)[0].clone();
    let source = stream_bytes(&doc, &Object::Ref(item.obj)).unwrap();
    let content_after_cm = |bytes: &[u8]| {
        let ops = pdfcraft_content::parse(bytes).ops;
        bytes[ops[1].span.end..].to_vec()
    };
    for rotation in [90, 180, 270, 0] {
        doc.update_dict(p, |d| d.set(b"Rotate".to_vec(), Object::Int(rotation))).unwrap();
        let before_lines = text::text_lines(&doc, 0).unwrap();
        let before_objects = editable_objects(&doc, 0).unwrap();
        let a = before_objects.iter().find(|o| o.target.kind == ObjectKind::Added).unwrap();
        // The logical added box must contain the actual source glyph geometry after rotation.
        let label = before_lines.iter().find(|l| l.text == "Added label").unwrap();
        assert!(
            label.rect[0] >= a.rect[0] - 0.01
                && label.rect[2] <= a.rect[2] + 0.01
                && label.rect[1] >= a.rect[1] - 0.01
                && label.rect[3] <= a.rect[3] + 0.01
        );
        move_objects(&mut doc, 0, &[a.target], [13.0, -17.0]).unwrap();
        let after_lines = text::text_lines(&doc, 0).unwrap();
        for (old, new) in before_lines.iter().zip(&after_lines) {
            let delta = if old.text == "Added label" { [13.0, -17.0] } else { [0.0, 0.0] };
            assert_eq!(old.text, new.text);
            assert!(close(new.rect, [old.rect[0] + delta[0], old.rect[1] + delta[1], old.rect[2] + delta[0], old.rect[3] + delta[1]]));
        }
        let current = added::list_added(&doc)[0].clone();
        let bytes = stream_bytes(&doc, &Object::Ref(current.obj)).unwrap();
        assert_eq!(content_after_cm(&bytes), content_after_cm(&source), "all drawing bytes after the placement, including glyphs, remain identical");
        assert_eq!(pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").cloned(), resources);
        doc = reopen(&doc);
    }
}

#[test]
fn group_moves_of_shared_added_streams_change_only_the_selected_page() {
    let mut doc = fixture();
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Shared added label".into(), rect: [40.0, 400.0, 240.0, 420.0], ..Default::default() }),
    )
    .unwrap();
    let pages = pdfcraft_model::pages(&doc);
    let contents = pages[0].dict.get(b"Contents").cloned().unwrap();
    doc.update_dict(pages[1].obj, |d| d.set(b"Contents".to_vec(), contents)).unwrap();
    let doc = reopen(&doc);
    let mut doc = doc;
    let other = page_content_bytes(&doc, 1);
    let before = editable_objects(&doc, 0).unwrap();
    let target = before.iter().find(|o| o.target.kind == ObjectKind::Added).unwrap().target;
    move_objects(&mut doc, 0, &[target], [15.0, -20.0]).unwrap();
    assert_eq!(page_content_bytes(&doc, 1), other, "a shared stream is copied on write");
    assert_ne!(added::list_added(&doc).iter().find(|a| a.page == 0).unwrap().obj, added::list_added(&doc).iter().find(|a| a.page == 1).unwrap().obj);
}

#[test]
fn group_moves_handle_added_content_under_inherited_graphics_state() {
    let mut doc = text_page("2 0 0 2 10 20 cm BT /F1 12 Tf 40 250 Td (Native) Tj ET");
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Added label".into(), rect: [50.0, 100.0, 200.0, 130.0], ..Default::default() }),
    )
    .unwrap();
    let before = text::text_lines(&doc, 0).unwrap();
    let target = editable_objects(&doc, 0).unwrap().iter().find(|o| o.target.kind == ObjectKind::Added).unwrap().target;
    move_objects(&mut doc, 0, &[target], [30.0, -20.0]).unwrap();
    let after = text::text_lines(&doc, 0).unwrap();
    for (old, new) in before.iter().zip(&after) {
        let delta = if old.text == "Added label" { [30.0, -20.0] } else { [0.0, 0.0] };
        assert!(close(new.rect, [old.rect[0] + delta[0], old.rect[1] + delta[1], old.rect[2] + delta[0], old.rect[3] + delta[1]]));
    }
}

#[test]
fn group_selection_refuses_incomplete_content_and_resource_budget_overruns() {
    for data in [
        format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "q ".repeat(1025)),
        format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "n ".repeat(100001)),
    ] {
        let mut doc = text_page(&data);
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(editable_objects(&doc, 0).is_err());
        assert!(move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [10.0, 10.0]).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td (Target) Tj ET");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(999999, 0)))).unwrap();
    assert!(editable_objects(&doc, 0).is_err(), "a missing stream cannot produce a partial inventory");
    let mut filter = Dict::new();
    filter.set(b"Filter".to_vec(), Object::name("NotAFilter"));
    let broken = doc.add(Object::Stream(Stream::from_raw(filter, vec![1, 2, 3])));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(broken))).unwrap();
    assert!(editable_objects(&doc, 0).is_err(), "an undecodable stream is refused");
}

#[test]
fn group_move_budget_refusals_keep_the_original_page_selectable() {
    let text = text_page(&format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "n ".repeat(99_995)));
    assert_eq!(pdfcraft_content::parse(&page_content_bytes(&text, 0)).ops.len(), 100_000);
    let mut image = image_page();
    let page = pdfcraft_model::pages(&image)[0].obj;
    let bytes = format!("{} /Im0 Do {}", "q ".repeat(1024), "Q ".repeat(1024));
    let content = image.add(Object::Stream(Stream::flate(Dict::new(), bytes.as_bytes())));
    image.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    for mut doc in [text, image] {
        let objects = editable_objects(&doc, 0).unwrap();
        assert_eq!(objects.len(), 1);
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let error = move_objects(&mut doc, 0, &[objects[0].target], [10.0, 5.0]).unwrap_err();
        assert!(error.to_string().contains("too many content operations") || error.to_string().contains("nested too deeply"), "{error}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
        assert_eq!(editable_objects(&doc, 0).unwrap(), objects, "refusal retains a usable inventory");
    }
}

#[test]
fn arabic_actual_text_keeps_typing_order_when_opened_for_editing() {
    for logical in ["مرحبا بالعالم", "سَلَام 123 (PDF)", "English مرحبا ١٢٣", "PDFمرحبا", "مرحبا", "שלום עולם", "Latin text"]
    {
        let hex: String = logical.encode_utf16().map(|u| format!("{u:04X}")).collect();
        for drawing in ["(abcd) Tj (efgh) Tj", "(X) Tj"] {
            let doc = text_page(&format!("/Span <</ActualText <FEFF{hex}>>> BDC BT /F1 12 Tf 72 700 Td {drawing} ET EMC"));
            for doc in [&doc, &reopen(&doc)] {
                let lines = text::text_lines(doc, 0).unwrap();
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].text, logical, "ActualText is already in typing order");
                assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, logical);
            }
        }
    }
}

#[test]
fn latin_ligature_actual_text_stays_on_its_line() {
    // Word and Chrome wrap ligatures in ActualText spans mid-word ("ﬁ" drawn, "fi" meant).
    let hex: String = "fi".encode_utf16().map(|u| format!("{u:04X}")).collect();
    let doc = text_page(&format!("BT /F1 12 Tf 72 700 Td (de) Tj /Span <</ActualText <FEFF{hex}>>> BDC (X) Tj EMC (ned well) Tj ET"));
    for doc in [&doc, &reopen(&doc)] {
        let lines = text::text_lines(doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["defined well"]);
        assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, "defined well");
    }
}

/// #3110: `/ActualText` may come from a named property list (`/Span /MC0 BDC`) looked up in the
/// page's or the form's `/Properties`, as well as from an inline one.
#[test]
fn actual_text_from_named_property_list() {
    let properties = "/Properties << /MC0 << /ActualText (Hello World) >> /MC1 << /Lang (en) >> >>";
    let page_with = |content: &str| {
        build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> {properties} >> /Contents 4 0 R >>"),
            &stream("", content),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        ])
    };
    for (content, expected) in [
        ("/Span /MC0 BDC BT /F1 24 Tf 72 700 Td (Glyphs) Tj ET EMC", "Hello World"),
        ("/Span <</ActualText (Hello World)>> BDC BT /F1 24 Tf 72 700 Td (Glyphs) Tj ET EMC", "Hello World"),
        ("/Span /MC1 BDC BT /F1 24 Tf 72 700 Td (Glyphs) Tj ET EMC", "Glyphs"),
        ("/Span /Missing BDC BT /F1 24 Tf 72 700 Td (Glyphs) Tj ET EMC", "Glyphs"),
    ] {
        let doc = page_with(content);
        for doc in [&doc, &reopen(&doc)] {
            let lines = text::text_lines(doc, 0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [expected], "{content}");
            assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, expected, "{content}");
            let reading: Vec<String> = text::reading_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect();
            assert_eq!(reading, [expected], "{content}");
        }
    }

    // A form's named list is looked up in the form's own /Properties, not the page's.
    let fm = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> /Properties << /MC0 << /ActualText (Form text) >> >> >>",
        "/Span /MC0 BDC BT /F1 24 Tf 72 600 Td (Glyphs) Tj ET EMC",
    );
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> {properties} >> /Contents 4 0 R >>"
        ),
        &stream("", "/Fm1 Do"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        &fm,
    ]);
    let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(reading, ["Form text"]);
}

#[test]
fn actual_text_and_visual_rtl_glyphs_keep_their_own_reading_order() {
    let hex: String = "שלום".encode_utf16().map(|u| format!("{u:04X}")).collect();
    let doc = hebrew_page(&format!("BT /F1 12 Tf 72 700 Td (EDCBA) Tj /Span <</ActualText <FEFF{hex}>>> BDC (FGHD) Tj EMC (EDCBA) Tj ET"));
    for doc in [&doc, &reopen(&doc)] {
        let lines = text::text_lines(doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["בדיוק", "שלום", "בדיוק"]);
    }
}

/// Synthetic codes A س, B م and X a lam-alef glyph, drawn from left to right.
fn arabic_ligature_page(content: &str, composite: bool) -> Document {
    let prefix = if composite { "00" } else { "" };
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
         1 begincodespacerange <{prefix}00> <{prefix}FF> endcodespacerange 4 beginbfchar \
         <{prefix}20> <0020> <{prefix}41> <0633> <{prefix}42> <0645> <{prefix}58> <FEFB> \
         endbfchar endcmap CMapName currentdict /CMap defineresource pop end end"
    );
    let widths: Vec<_> = (32..=88).map(|c| if c == 32 { "250" } else { "500" }).collect();
    let font = if composite {
        "<< /Type /Font /Subtype /Type0 /BaseFont /FakeArabic /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 6 0 R >>".into()
    } else {
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /FakeArabic /FirstChar 32 /LastChar 88 /Widths [{}] /ToUnicode 6 0 R >>", widths.join(" "))
    };
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream("", content),
        &font,
        &stream("", &cmap),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /FakeArabic /CIDSystemInfo << /Registry (Synthetic) /Ordering (Identity) /Supplement 0 >> /DW 500 /W [32 [250]] >>",
    ])
}

#[test]
fn arabic_ligature_actual_text_stays_on_its_line() {
    for (ligature, composite) in [("لا", false), ("لأ", false), ("لإ", false), ("لآ", false), ("لَا", false), ("لا", true)] {
        let hex: String = ligature.encode_utf16().map(|u| format!("{u:04X}")).collect();
        let (left, glyph, right) = if composite { ("<0042>", "<0058>", "<0041>") } else { ("(B)", "(X)", "(A)") };
        let doc = arabic_ligature_page(
            &format!("BT /F1 12 Tf 72 700 Td {left} Tj /Span <</ActualText <FEFF{hex}>>> BDC [{glyph}] TJ EMC {right} Tj ET"),
            composite,
        );
        let expected = format!("س{ligature}م");
        for doc in [&doc, &reopen(&doc)] {
            let lines = text::text_lines(doc, 0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [expected.as_str()]);
            assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, expected);
        }
    }
}

#[test]
fn multi_glyph_rtl_actual_text_keeps_its_own_reading_order() {
    // Two encoded glyphs are a logical span, even if their replacement looks like a ligature.
    for drawing in ["(XX) Tj", "(X) Tj (X) Tj", "[(X) (X)] TJ"] {
        let doc =
            arabic_ligature_page(&format!("BT /F1 12 Tf 72 700 Td (B) Tj /Span <</ActualText <FEFF06440627>>> BDC {drawing} EMC (A) Tj ET"), false);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["م", "لا", "س"]);
    }
}

#[test]
fn replacing_word_with_actual_text_ligature_removes_all_its_source_operators() {
    let mut doc = arabic_ligature_page(
        "BT /F1 12 Tf 72 700 Td (B) Tj /Span <</ActualText <FEFF06440627>>> BDC /Span BMC (X) Tj EMC EMC (A) Tj ET \
         BT /F1 12 Tf 72 600 Td (AB) Tj ET",
        false,
    );
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, "سلام");
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Replacement", "مس"]);
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(!content.contains("ActualText") && !content.contains("(X)") && !content.contains("(B)") && !content.contains("(A)"), "{content}");
}

#[test]
fn replacing_arabic_actual_text_removes_the_whole_original_drawing() {
    let logical = "مرحبا بالعالم";
    let hex: String = logical.encode_utf16().map(|u| format!("{u:04X}")).collect();
    let mut doc = text_page(&format!(
        "/Span <</ActualText <FEFF{hex}>>> BDC BT /F1 12 Tf 72 700 Td (abcd) Tj (efgh) Tj ET EMC \
         BT /F1 12 Tf 72 600 Td (Keep this line) Tj ET"
    ));
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, logical);
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Replacement", "Keep this line"]);
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(!content.contains("ActualText") && !content.contains("(abcd)") && !content.contains("(efgh)"));
}

fn mixed_region_page() -> Document {
    let mut doc = text_page(
        "BT /F2 1 Tf 18 0 0 18 123 700 Tm (TITLE) Tj ET \
         BT /F2 1 Tf 12 0 0 12 117.6 676 Tm (two lines) Tj ET \
         BT /F2 1 Tf 12 0 0 12 135.6 652 Tm (last) Tj ET \
         BT 0 0.2 0.5 rg /F2 10 Tf 72 600 Td (About) Tj ET \
         BT 0 g /F1 10 Tf 72 582 Td (Plain ) Tj /F2 10 Tf 0 0.2 0.5 rg (bold) Tj /F1 10 Tf 0 g ( text.) Tj ET \
         BT 0.2 0.4 0.7 rg /F3 8 Tf 72 566 Td (Italic footer) Tj ET \
         BT 0 g /F1 10 Tf 400 600 Td (Other column) Tj ET",
    );
    for (obj, base) in [(5, "Courier"), (6, "Courier-Bold")] {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        doc.set(pdfcraft_cos::ObjRef::new(obj, 0), Object::Dict(f));
    }
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name("Courier-Oblique"));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let font = doc.add(Object::Dict(f));
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F3".to_vec(), Object::Ref(font));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    doc
}

#[test]
fn mixed_styles_and_matrix_sizes_form_visual_regions_without_merging_columns() {
    let doc = mixed_region_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].editor_text, "TITLE\ntwo lines\nlast");
    assert_eq!(blocks[0].rows.len(), 3);
    assert_eq!(blocks[1].editor_text, "About\nPlain bold text.\nItalic footer");
    assert_eq!(blocks[1].rows.len(), 3);
    assert_eq!(blocks[1].lines.len(), 5);
    assert_eq!(blocks[2].text, "Other column");
}

#[test]
fn a_single_row_with_mixed_styles_grows_before_wrapping() {
    let mut doc = text_page("BT /F1 10 Tf 72 582 Td (Plain ) Tj /F2 10 Tf (bold) Tj /F1 10 Tf ( text.) Tj ET");
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |f| {
        *f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name("Helvetica-Bold"));
    })
    .unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let index = blocks.iter().position(|b| b.editor_text == "Plain bold text.").unwrap();
    let source = &blocks[index];
    assert_eq!(source.rows.len(), 1);
    assert!(source.lines.len() > 1);
    let draft = "Plain bold text. More words on the same line";
    text::replace_block(&mut doc, 0, index, draft).unwrap();
    let edited = text::text_blocks(&reopen(&doc), 0).unwrap();
    let block = edited.iter().find(|b| b.text == draft).unwrap();
    assert_eq!(block.rows.len(), 1, "a style boundary must not pin a single row to its original width");
    assert!((block.rect[0] - source.rect[0]).abs() < 0.1);
    assert!(block.rect[2] > source.rect[2] + 20.0);
}

#[test]
fn explicit_editor_rows_survive_single_run_edit_and_reopen() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original heading) Tj ET");
    let text = "First row\nSecond row\nLast row";
    let layout = text::text_edit_layout(&doc, 0, 0, text, &text::BlockStyle::default()).unwrap();
    text::replace_block(&mut doc, 0, 0, text).unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks[0].editor_text, text);
    assert_eq!(blocks[0].rows.len(), 3);
    assert_eq!(Some(blocks[0].rect), layout.draft_rect);
}

#[test]
fn rich_region_edits_preserve_styles_positions_and_delete_all_runs_after_reopen() {
    let original = mixed_region_page();
    let before = text::text_lines(&original, 0).unwrap();
    let block = text::text_blocks(&original, 0).unwrap()[1].clone();
    let mut doc = original.clone();
    text::replace_block(&mut doc, 0, 1, &block.editor_text.replace("Plain", "Other")).unwrap();
    let reopened = reopen(&doc);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[1].editor_text, "About\nOther bold text.\nItalic footer");
    let lines = text::text_lines(&reopened, 0).unwrap();
    for word in ["About", "bold", "Italic footer"] {
        let source = before.iter().find(|l| l.text == word).unwrap();
        let after = lines.iter().find(|l| l.text == word).unwrap();
        assert_eq!(
            (after.base_font.as_str(), after.size, after.color, after.bold, after.italic),
            (source.base_font.as_str(), source.size, source.color, source.bold, source.italic)
        );
        assert!((after.origin_baseline() - source.origin_baseline()).abs() < 0.001);
    }
    for i in [0, 2] {
        assert_eq!(blocks[i].editor_text, text::text_blocks(&original, 0).unwrap()[i].editor_text);
        assert_eq!(blocks[i].rect, text::text_blocks(&original, 0).unwrap()[i].rect);
    }
    let mut removed = original.clone();
    text::replace_block(&mut removed, 0, 1, "").unwrap();
    let after = text::text_blocks(&reopen(&removed), 0).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].editor_text, "TITLE\ntwo lines\nlast");
    assert_eq!(after[1].text, "Other column");
    let mut preview = original.clone();
    text::clear_block(&mut preview, 0, 1).unwrap();
    assert_eq!(text::text_blocks(&reopen(&preview), 0).unwrap().len(), 2);
    assert_eq!(text::text_blocks(&original, 0).unwrap().len(), 3);
}

/// Two edits applied together (heading and footer edited before leaving the editor, #2198): the
/// runs between them are unchanged and keep their own styles instead of taking the heading's.
#[test]
fn unchanged_runs_between_two_edits_keep_their_styles() {
    let original = mixed_region_page();
    let look =
        |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.trim().to_string(), l.base_font, l.color)).collect::<Vec<_>>();
    let before = look(&original);
    let block = text::text_blocks(&original, 0).unwrap()[1].clone();
    let mut doc = original.clone();
    let draft = block.editor_text.replace("About", "About us").replace("footer", "footnote");
    text::replace_block(&mut doc, 0, 1, &draft).unwrap();
    let after = look(&reopen(&doc));
    let style = |lines: &[(String, String, [f64; 3])], word: &str| lines.iter().find(|l| l.0 == word).map(|l| (l.1.clone(), l.2));
    for word in ["Plain", "bold", "text."] {
        assert_eq!(style(&after, word), style(&before, word), "{word:?} keeps its own style; lines after the edit: {after:?}");
    }
    assert_eq!(style(&after, "About us"), style(&before, "About"));
    assert_eq!(style(&after, "Italic footnote"), style(&before, "Italic footer"));
}

#[test]
fn disjoint_style_ranges_preserve_unselected_runs_and_validate_utf8_atomically() {
    let mut doc = mixed_region_page();
    let block = text::text_blocks(&doc, 0).unwrap()[1].clone();
    let a = block.editor_text.find("Plain").unwrap();
    let b = block.editor_text.find("bold").unwrap();
    let style = BlockStyle {
        ranges: vec![
            RangeStyle {
                range: a..a + 5,
                family: None,
                resource: None,
                size: Some(15.0),
                color: Some([0.8, 0.1, 0.2]),
                underline: Some(true),
                char_spacing: None,
                scale: None,
            },
            RangeStyle {
                range: b..b + 4,
                family: Some((Family::Courier, false, true)),
                resource: None,
                size: None,
                color: None,
                underline: None,
                char_spacing: Some(0.2),
                scale: Some(90.0),
            },
        ],
        width: Some(300.0),
        ..Default::default()
    };
    text::rewrite_block(&mut doc, 0, 1, Some(&block.editor_text), &style).unwrap();
    doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let plain = lines.iter().find(|l| l.text == "Plain").unwrap();
    assert_eq!((plain.size, plain.color), (15.0, [0.8, 0.1, 0.2]));
    let bold = lines.iter().find(|l| l.text == "bold").unwrap();
    assert_eq!(bold.base_font, "Courier-Oblique");
    let original = mixed_region_page();
    for word in ["About", "Italic footer"] {
        let prior = text::text_lines(&original, 0).unwrap().into_iter().find(|l| l.text == word).unwrap();
        let after = lines.iter().find(|l| l.text == word).unwrap();
        assert_eq!((after.size, after.color, &after.font, after.baseline()), (prior.size, prior.color, &prior.font, prior.baseline()));
    }
    let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    let invalid = BlockStyle {
        ranges: vec![RangeStyle {
            range: 1..2,
            family: None,
            resource: None,
            size: Some(12.0),
            color: None,
            underline: None,
            char_spacing: None,
            scale: None,
        }],
        ..Default::default()
    };
    assert!(text::rewrite_block(&mut doc, 0, 1, Some("é bad"), &invalid).is_err());
    assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn centered_mixed_size_title_keeps_three_rows_when_changed() {
    let mut doc = mixed_region_page();
    let title = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::replace_block(&mut doc, 0, 0, &title.editor_text.replace("last", "tail")).unwrap();
    let reopened = reopen(&doc);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks[0].editor_text, "TITLE\ntwo lines\ntail");
    assert_eq!(blocks[0].rows.len(), 3);
    let lines = text::text_lines(&reopened, 0).unwrap();
    for row in &blocks[0].rows {
        let line = &lines[row[0]];
        assert!(((line.rect[0] + line.rect[2]) / 2.0 - 150.0).abs() < 0.01);
    }
}

#[test]
fn image_selection_bounds_respect_clips_and_restore_graphics_state() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let content = b"q 1 0 0 1 10 20 cm 20 30 100 80 re W* n q 1200 0 0 1400 -500 -600 cm /Im0 Do Q Q q 200 0 0 100 100 100 cm /Im0 Do Q q 0 0 10 10 re W n 100 0 0 100 200 200 cm /Im0 Do Q";
    let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), content)));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    let before = images::page_images(&doc, 0).unwrap();
    assert_eq!(before.len(), 3, "fully clipped placements retain source indexes");
    assert_eq!(before[0].visible_rect, Some([30.0, 50.0, 130.0, 130.0]));
    assert_eq!(before[1].visible_rect, Some(before[1].rect), "Q restores the prior clip");
    assert_eq!(before[2].visible_rect, None);
    let objects = crate::editable_objects(&doc, 0).unwrap();
    assert_eq!(objects.len(), 2, "hidden images cannot intercept selection");
    assert_eq!(objects[0].rect, before[0].visible_rect.unwrap());
    let offset = [12.0, 14.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(pdfcraft_content::Matrix::translate(offset[0], offset[1]).0)).unwrap();
    let reopened = reopen(&doc);
    let after = images::page_images(&reopened, 0).unwrap();
    assert_eq!(after[0].visible_rect, Some([42.0, 64.0, 142.0, 144.0]), "an exclusive crop moves with the image even across nested graphics states");
    for (a, b) in after[0].rect.iter().zip(before[0].rect.iter()).enumerate() {
        assert!((b.0 - b.1 - offset[a % 2]).abs() < 0.001);
    }
    assert_eq!(after[1].rect, before[1].rect, "neighbor placement unchanged");
    assert_eq!(after[1].visible_rect, before[1].visible_rect);
    assert_eq!(after[1].matrix, before[1].matrix);
    assert_eq!(after[1].object, before[1].object);
}

#[test]
fn image_clip_path_and_graphics_state_continue_across_content_streams() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let pieces: [&[u8]; 3] = [b"q 20 30 40 50 re W", b"n 1000 0 0 1000 -300 -400 cm /Im0 Do", b"Q q 50 0 0 50 100 200 cm /Im0 Do Q"];
    let refs: Vec<_> = pieces.into_iter().map(|b| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
    let images = images::page_images(&doc, 0).unwrap();
    assert_eq!(images[0].visible_rect, Some([20.0, 30.0, 60.0, 80.0]));
    assert_eq!(images[1].visible_rect, Some([100.0, 200.0, 150.0, 250.0]));
    images::change_image(
        &mut doc,
        0,
        0,
        &images::ImageChange::Transform(images::rect_to_rect(images[0].visible_rect.unwrap(), [20.0, 30.0, 80.0, 105.0])),
    )
    .unwrap();
    let after = images::page_images(&reopen(&doc), 0).unwrap();
    assert!(close(after[0].visible_rect.unwrap(), [20.0, 30.0, 80.0, 105.0]), "an exclusive crop scales with its image across streams");
    assert_eq!(after[1].matrix, images[1].matrix, "graphics state restored across stream boundaries");
}

#[test]
fn cropped_image_resizing_moves_private_clips_and_keeps_shared_ones() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let stream = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 30 170 130 80 re W* n 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET",
    )));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    for target in [[30.0, 170.0, 190.0, 270.0], [30.0, 170.0, 110.0, 220.0], [70.0, 210.0, 150.0, 260.0]] {
        let before = images::page_images(&doc, 0).unwrap()[0].clone();
        let transform = images::rect_to_rect(before.visible_rect.unwrap(), target);
        images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
        doc = reopen(&doc);
        let after = images::page_images(&doc, 0).unwrap()[0].clone();
        // The moved clip's `cm` is written with the content stream's number precision.
        let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05);
        assert!(near(after.visible_rect.unwrap(), target), "{:?} vs {target:?}", after.visible_rect);
        assert!(near(after.rect, pdfcraft_content::Matrix(transform).bbox(before.rect)), "full pixels use the same crop transform");
        assert_eq!(after.object, before.object, "pixels are never resampled or replaced");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].baseline(), [30.0, 130.0]);
    }
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let stream = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 30 170 130 80 re W n q 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 40 200 Td (Shared caption) Tj ET Q",
    )));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    let before = images::page_images(&doc, 0).unwrap();
    let caption = text::text_lines(&doc, 0).unwrap();
    let transform = [1.2, 0.0, 0.0, 1.2, 0.0, 0.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
    doc = reopen(&doc);
    let after = images::page_images(&doc, 0).unwrap();
    assert!(close(after[0].visible_rect.unwrap(), before[0].visible_rect.unwrap()), "a shared clip stays where it is");
    assert_eq!(after[0].object, before[0].object);
    assert_eq!(
        text::text_lines(&doc, 0).unwrap().iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
        caption.iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>()
    );
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform([0.5, 0.0, 0.0, 0.5, 0.0, 0.0])).unwrap();
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 1);
}

/// Report generators draw each picture as `q re W n cm Do Q` inside a section's clip that the
/// section's text shares. Resizing the picture scales its own clip with it (the enlarged picture
/// used to show only through its old box) while the section's clip and text stay put.
#[test]
fn resizing_an_image_under_a_shared_clip_moves_its_own_clip() {
    // The picture's own clip as wide as the picture, and wider (both seen in one report).
    for clip_width in [61.2, 85.0] {
        let mut doc = image_page_with(&format!(
            "q 20 20 560 760 re W n q 162 79 {clip_width} 28.3 re W n 61.2 0 0 28.3 162 79 cm /Im0 Do Q BT /F1 12 Tf 30 50 Td (Section text) Tj ET Q"
        ));
        let before = images::page_images(&doc, 0).unwrap()[0].clone();
        assert!(close(before.visible_rect.unwrap(), [162.0, 79.0, 223.2, 107.3]), "{:?}", before.visible_rect);
        let caption = text::text_lines(&doc, 0).unwrap();
        let target = [162.0, 79.0, 284.4, 135.6];
        let transform = images::rect_to_rect(before.visible_rect.unwrap(), target);
        images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
        doc = reopen(&doc);
        let after = images::page_images(&doc, 0).unwrap()[0].clone();
        let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05);
        assert!(near(after.visible_rect.unwrap(), target), "clip {clip_width}: the whole enlarged picture shows, {:?}", after.visible_rect);
        assert_eq!(after.object, before.object, "pixels are never resampled or replaced");
        assert_eq!(
            text::text_lines(&doc, 0).unwrap().iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
            caption.iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
            "the section's text stays where it was"
        );
        // The section's clip is shared, so it still bounds the picture: past it, nothing shows.
        let past = images::rect_to_rect(after.visible_rect.unwrap(), [500.0, 79.0, 700.0, 135.6]);
        images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(past)).unwrap();
        let clipped = images::page_images(&reopen(&doc), 0).unwrap()[0].visible_rect.unwrap();
        assert!(clipped[2] <= 580.05, "the section's clip stays in force: {clipped:?}");
    }
}

#[test]
fn private_image_crop_with_intervening_state_resizes_across_streams() {
    let variants: Vec<Vec<&[u8]>> = vec![
        vec![b"q 1 0 0 1 10 20 cm 20 150 130 80 re W* n 0.5 g q 800 0 0 900 -300 -400 cm /Im0 Do Q Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET"],
        vec![b"q 1 0 0 1 10 20 cm 20 150 130 80 re W*", b"n 0.5 g q 800 0 0 900 -300 -400 cm /Im0 Do", b"Q Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET"],
    ];
    for pieces in variants {
        let mut doc = image_page();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let refs = pieces.iter().map(|b| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
        let caption = text::text_lines(&doc, 0).unwrap();
        for target in [[30.0, 170.0, 190.0, 270.0], [30.0, 170.0, 110.0, 220.0], [70.0, 210.0, 150.0, 260.0]] {
            let before = images::page_images(&doc, 0).unwrap()[0].clone();
            let transform = images::rect_to_rect(before.visible_rect.unwrap(), target);
            images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
            doc = reopen(&doc);
            let after = images::page_images(&doc, 0).unwrap()[0].clone();
            assert!(close(after.visible_rect.unwrap(), target), "the private crop follows the image without a literal operator pattern");
            assert!(close(after.rect, pdfcraft_content::Matrix(transform).bbox(before.rect)));
            assert_eq!(after.object, before.object);
            assert_eq!(
                text::text_lines(&doc, 0).unwrap().iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
                caption.iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>()
            );
        }
    }
}

/// [`image_page`] with `pieces` as its `/Contents` array and a Form /Fm0 drawing 72,500–172,600.
fn split_image_page(pieces: &[&[u8]]) -> Document {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([72, 500, 172, 600].map(Object::Int).to_vec()));
    let form = doc.add(Object::Stream(Stream::flate(form, b"0.5 g 72 500 100 100 re f")));
    let image = doc.get(pdfcraft_model::pages(&doc)[0].obj);
    let mut res = image.as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| r.as_dict().cloned()).unwrap();
    let mut xo = res.get(b"XObject").and_then(|x| x.as_dict().cloned()).unwrap();
    xo.set(b"Fm0".to_vec(), Object::Ref(form));
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    let refs = pieces.iter().map(|b| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
    doc.update_dict(page, |d| {
        d.set(b"Contents".to_vec(), Object::Array(refs));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    doc
}

#[test]
fn an_image_whose_operator_starts_the_next_stream_can_be_selected_moved_and_deleted() {
    // #3113: an operator's operands end one piece and its keyword starts the next.
    let square = [72.0, 500.0, 172.0, 600.0];
    let cases: [(&[&[u8]], bool); 4] = [
        (&[b"q 100 0 0 100 72 500 cm /Im0", b"Do Q"], false),
        (&[b"q 100 0 0 100 72 500", b"cm /Im0 Do Q"], false),
        (&[b"q /Fm0", b"Do Q"], true),
        (&[b"q 100 0 0 100 72 500 cm /Im0 Do Q"], false),
    ];
    for (pieces, is_form) in cases {
        let doc = split_image_page(pieces);
        let label: Vec<_> = pieces.iter().map(|p| String::from_utf8_lossy(p)).collect();
        let images = images::page_images(&doc, 0).unwrap();
        assert_eq!(images.len(), 1, "{label:?}");
        assert!(close(images[0].rect, square), "{label:?}: {:?}", images[0].rect);
        assert_eq!(images[0].is_form, is_form);
        assert!(images[0].contains([122.0, 550.0]), "{label:?}");
        if !is_form {
            let read = images::reading_images(&doc, 0).unwrap();
            assert!(read.len() == 1 && close(read[0].rect, square), "{label:?}: {read:?}");
        }

        let mut deleted = doc.clone();
        images::change_image(&mut deleted, 0, 0, &images::ImageChange::Delete).unwrap();
        let deleted = reopen(&deleted);
        assert!(images::page_images(&deleted, 0).unwrap().is_empty(), "{label:?}");
        let (joined, ops) = joined_ops(&deleted);
        assert!(!ops.iter().any(|o| o.is("Do")), "{joined}");

        let target = [300.0, 500.0, 400.0, 600.0];
        let mut moved = doc.clone();
        images::change_image(&mut moved, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(square, target))).unwrap();
        let moved = reopen(&moved);
        let after = images::page_images(&moved, 0).unwrap();
        assert!(after.len() == 1 && close(after[0].rect, target), "{label:?}: {after:?} {:?}", joined_ops(&moved).0);

        let mut shifted = doc.clone();
        images::translate_images(&mut shifted, 0, &[0], [228.0, 0.0]).unwrap();
        let shifted = reopen(&shifted);
        let after = images::page_images(&shifted, 0).unwrap();
        assert!(after.len() == 1 && close(after[0].rect, target), "{label:?}: {after:?} {:?}", joined_ops(&shifted).0);
    }
}

#[test]
fn curved_image_clips_have_analytic_bounds_and_holes_do_not_capture_points() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let bytes =
        b"q 0 0 m 0 200 100 200 100 0 c h W n 300 0 0 300 0 0 cm /Im0 Do Q q 100 100 100 100 re 125 125 50 50 re W* n 400 0 0 400 0 0 cm /Im0 Do Q";
    let content = doc.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    let images = images::page_images(&doc, 0).unwrap();
    assert!(close(images[0].visible_rect.unwrap(), [0.0, 0.0, 100.0, 150.0]), "cubic extrema, not controls");
    assert!(images[0].contains([50.0, 100.0]));
    assert!(!images[0].contains([2.0, 140.0]), "empty corners of curve bounds are not artwork");
    assert!(images[1].contains([110.0, 110.0]));
    assert!(!images[1].contains([150.0, 150.0]), "even-odd hole");
    images::change_image(&mut doc, 0, 1, &images::ImageChange::Transform([1.2, 0.0, 0.0, 1.2, 10.0, 15.0])).unwrap();
    let after = images::page_images(&reopen(&doc), 0).unwrap();
    assert!(!after[1].contains([190.0, 195.0]), "the transformed even-odd hole remains empty");
    assert!(after[1].contains([142.0, 147.0]), "the transformed filled corner remains selectable");
    assert_eq!(after[0].visible_rect, images[0].visible_rect, "the neighbouring curved clip stays put");
}

#[test]
fn marked_paragraph_boundaries_and_cross_stream_transforms_survive_rewrite() {
    let mut doc =
        text_page("/P << /MCID 0 >> BDC BT /F1 12 Tf 40 500 Td (One) Tj ET EMC /P << /MCID 1 >> BDC BT /F1 12 Tf 40 482 Td (Two) Tj ET EMC");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "distinct explicit paragraphs win over nearby geometry");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let parts: [&[u8]; 2] =
        [b"/P << /MCID 2 >> BDC BT /F1 12 Tf 40 500 Td (First row) Tj ET", b"q 1 0 0 1 10 12 cm BT /F1 12 Tf 30 470 Td (Second row) Tj ET Q EMC"];
    let refs = parts.iter().map(|p| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), p))))).collect();
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
    let block = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(block.len(), 1, "one marked paragraph across streams and coordinate spaces");
    assert_eq!(block[0].editor_text, "First row\nSecond row");
    let original = text::text_lines(&doc, 0).unwrap();
    text::rewrite_block(&mut doc, 0, 0, Some("First row\nChanged row"), &text::BlockStyle { width: Some(200.0), ..Default::default() }).unwrap();
    let after = text::text_lines(&reopen(&doc), 0).unwrap();
    for (a, b) in after.iter().zip(original) {
        assert!(close(a.rect, b.rect) || a.baseline() == b.baseline());
    }
    assert_eq!(after[1].text, "Changed row");
}

#[test]
fn logical_indic_with_nuktas_not_reordered_on_standard_font() {
    // Already logical Hindi text with nuktas (बढ़िया) must not be corrupted by pre-base reordering
    // when the font does not require visual order.
    let hex_actual = "<FEFF092C0922093C093F092F093E>";
    let doc = text_page(&format!("/Span << /ActualText {hex_actual} >> BDC BT /F1 12 Tf 40 500 Td (ab) Tj ET EMC"));
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    assert!(!lines[0].visual_order);
}

/// One page in a font named `base_font` whose ToUnicode maps A ब, B ि, C ढ़, D य, E ा, F क, G त.
/// A legacy visual-order font draws "बढ़िया" as `(ABCDE)` (ब ि ढ़ या) and "किताब" as `(BFGEA)`.
fn devanagari_page(base_font: &str, content: &str) -> Document {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 8 beginbfchar \
                <20> <0020> <41> <092C> <42> <093F> <43> <0922093C> <44> <092F> <45> <093E> <46> <0915> <47> <0924> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let widths: Vec<&str> = (32..=71)
        .map(|c| {
            if c == 32 {
                "250"
            } else if c >= 65 {
                "500"
            } else {
                "0"
            }
        })
        .collect();
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /{base_font} /FirstChar 32 /LastChar 71 /Widths [{}] /ToUnicode 6 0 R >>",
        widths.join(" ")
    );
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream("", content),
        &font,
        &stream("", cmap),
    ])
}

/// A legacy visual-order font (Kruti Dev) whose ToUnicode gives the glyphs in drawing order: the
/// pre-base vowel sign is read back after its consonant, for editing and for reading.
#[test]
fn legacy_indic_font_flags_visual_order_and_reorders_pre_base_vowels() {
    let doc = devanagari_page("KrutiDev010", "BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].visual_order);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(reading, ["\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}"]);
}

/// Typed (logical) text is written back in the font's visual order and reads back as typed.
#[test]
fn legacy_indic_font_edits_are_written_in_visual_order() {
    let mut doc = devanagari_page("ABCDEF+KrutiDev010", "BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET");
    let kitab = "\u{0915}\u{093F}\u{0924}\u{093E}\u{092C}";
    assert_eq!(text::replace_line(&mut doc, 0, 0, kitab).unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(BFGEA)"), "{content}");
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, kitab);
}

/// `/ActualText` is already in logical order, even in a visual-order font: "बढ़िया" must not
/// become "बढ़यिा".
#[test]
fn logical_actual_text_in_a_legacy_indic_font_is_not_reordered() {
    let doc = devanagari_page("KrutiDev010", "/Span << /ActualText <FEFF092C0922093C093F092F093E> >> BDC BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET EMC");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(reading, ["\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}"]);
}

/// A modern subset font with a logical ToUnicode and no GSUB in the PDF (subsets and bare CFF
/// often drop it) is not visual order: its text is read as it is.
#[test]
fn subset_indic_font_without_legacy_name_keeps_logical_text() {
    let doc = devanagari_page("ABCDEF+NotoSansDevanagari-Regular", "BT /F1 12 Tf 40 500 Td (ACBDE) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].visual_order);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
}

/// Lines of a paragraph drawn turned on the page (a landscape page drawn a quarter turn round)
/// are separate rows, and adding a word keeps them apart (#1443).
#[test]
fn turned_paragraph_keeps_its_line_breaks_when_a_word_is_added() {
    for cm in ["0 1 -1 0 600 0", "0 -1 1 0 0 800", "-1 0 0 -1 600 800"] {
        let original =
            text_page(&format!("q {cm} cm BT /F1 12 Tf 72 500 Td (First line here) Tj 0 -14 Td (Second line) Tj 0 -14 Td (Third) Tj ET Q"));
        let block = text::text_blocks(&original, 0).unwrap().remove(0);
        assert_eq!(block.editor_text, "First line here\nSecond line\nThird", "{cm}");
        let mut doc = original.clone();
        text::rewrite_block(&mut doc, 0, 0, Some("First line here\nSecond line\nThird added"), &text::BlockStyle::default()).unwrap();
        let reopened = reopen(&doc);
        assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].editor_text, "First line here\nSecond line\nThird added", "{cm}");
        let (before, after) = (text::text_lines(&original, 0).unwrap(), text::text_lines(&reopened, 0).unwrap());
        assert_eq!(after.len(), 3, "{cm}");
        for (a, b) in after.iter().zip(&before).take(2) {
            assert!(close(a.rect, b.rect), "{cm}: untouched rows stay put: {:?} {:?}", a.rect, b.rect);
        }
        // The last row grows along its own baseline from where it started.
        let (grown, old) = (after[2].rect, before[2].rect);
        assert!((0..2).all(|i| grown[i] <= old[i] + 0.01) && (2..4).all(|i| grown[i] >= old[i] - 0.01), "{cm}: {grown:?} {old:?}");
        assert!((after[2].origin_baseline() - before[2].origin_baseline()).abs() < 0.01, "{cm}");
    }
    // A turned single line grows rather than wrapping across the page's other axis.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 300 Tm (Only line) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("Only line with several more words added"), &text::BlockStyle::default()).unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks[0].editor_text, "Only line with several more words added");
    // Formatting part of turned text isn't laid out yet: refused, not drawn along the page's x axis.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 300 Tm (Only line) Tj ET");
    let range = text::RangeStyle {
        range: 0..4,
        family: None,
        resource: None,
        size: Some(20.0),
        color: None,
        underline: None,
        char_spacing: None,
        scale: None,
    };
    let style = text::BlockStyle { ranges: vec![range], ..Default::default() };
    assert!(text::rewrite_block(&mut doc, 0, 0, Some("Only line"), &style).is_err());
}

/// A paragraph drawn at a slant rewraps at the width its rows have along their baseline, as the
/// same paragraph upright does. Its upright bounding box turned into the baseline's frame is wider
/// by the rows' height times |sin 2θ|, which let a word that doesn't fit stay on its row.
#[test]
fn a_paragraph_at_a_slant_rewraps_at_the_width_it_has_upright() {
    let rows = ["Rotated paragraph, first row of text", "that continues on a second row and", "ends on the third row here."];
    let draft = "Rotated paragraph, first row of text that continues on a second row and now carries a few extra words \
        before it ends on the third row here.";
    let rewrapped = |degrees: f64| {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let shown = rows.iter().map(|r| format!("({r}) Tj")).collect::<Vec<_>>().join(" T* ");
        let mut doc = text_page(&format!("BT /F1 12 Tf 14.4 TL {cos:.6} {sin:.6} {:.6} {cos:.6} 300 400 Tm {shown} ET", -sin));
        let block = text::text_blocks(&doc, 0).unwrap().remove(0);
        assert_eq!(block.editor_text, rows.join("\n"), "{degrees}°");
        text::rewrite_block(&mut doc, 0, 0, Some(draft), &text::BlockStyle::default()).unwrap();
        text::text_lines(&reopen(&doc), 0).unwrap().into_iter().map(|l| l.text).collect::<Vec<_>>()
    };
    let upright = rewrapped(0.0);
    // The widest source row is 189.44 pt; "now carries a few extra words before" is 196.07 pt.
    assert_eq!(upright[2], "now carries a few extra words", "{upright:?}");
    for degrees in [30.0, 45.0, 60.0, 90.0, 135.0, 210.0, 330.0] {
        assert_eq!(rewrapped(degrees), upright, "{degrees}°");
    }
}

/// A paragraph in a Chrome-style subset font: `/W` leaves out the glyphs of the `/DW` width.
/// Adding a word keeps every row as it was and shows the word in the page's own font (#1443).
#[test]
fn default_width_subset_paragraph_keeps_its_rows_when_a_word_is_added() {
    let hex = |s: &str| s.chars().map(|c| format!("00{:02X}", u32::from(c))).collect::<String>();
    let rows = ["This is the first line", "the second", "and third"];
    let content = rows
        .iter()
        .enumerate()
        .map(|(i, row)| format!("BT /F3 12 Tf 1 0 0 1 72 {} Tm <{}> Tj ET", 700 - 14 * i, hex(row)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut doc = text_page(&content);
    let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
        1 beginbfrange <0020> <007E> <0020> endbfrange end";
    let to_unicode = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name("AAAAAA+LiberationSans"));
    cid.set(b"CIDToGIDMap".to_vec(), Object::name("Identity"));
    cid.set(b"DW".to_vec(), Object::Int(500));
    cid.set(
        b"W".to_vec(),
        Object::Array(vec![Object::Int(32), Object::Array(vec![Object::Int(278)]), Object::Int(84), Object::Array(vec![Object::Int(611)])]),
    );
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name("AAAAAA+LiberationSans"));
    font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid)]));
    font.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    let font = doc.add(Object::Dict(font));
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F3".to_vec(), Object::Ref(font));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let block = text::text_blocks(&doc, 0).unwrap().remove(0);
    assert_eq!(block.editor_text, rows.join("\n"));
    let before = text::text_lines(&doc, 0).unwrap();
    let edit = text::rewrite_block(&mut doc, 0, 0, Some(&format!("{} added", block.editor_text)), &text::BlockStyle::default()).unwrap();
    assert_eq!(edit.substituted, None, "the subset shows every letter of the new word");
    let reopened = reopen(&doc);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].editor_text, "This is the first line\nthe second\nand third added");
    let after = text::text_lines(&reopened, 0).unwrap();
    for (a, b) in after.iter().zip(&before).take(2) {
        assert_eq!(a.text, b.text);
        assert!(close(a.rect, b.rect), "untouched rows keep their place and width: {:?} {:?}", a.rect, b.rect);
    }
}

/// #2578: a single turned row grows along its baseline only up to the page's 36 pt margin and wraps
/// onto further rows from there, instead of running off the page.
#[test]
fn turned_single_row_wraps_at_the_page_margin() {
    let draft = "A single turned row that keeps growing with many more words added until it is far longer than the page is tall, so it has to stop or wrap somewhere";
    // Reading upwards (90° CCW) from y = 600 on the 600 × 800 test page: top margin is at y = 764.
    let before = text_page("BT /F1 12 Tf 0 1 -1 0 300 600 Tm (A single turned row) Tj ET");
    let layout = text::text_edit_layout(&before, 0, 0, draft, &text::BlockStyle::default()).unwrap();
    let mut doc = before;
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &text::BlockStyle::default()).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert!(lines.len() > 1, "expected the turned row to wrap at the top margin: {lines:?}");
    for line in &lines {
        assert!((line.rect[1] - 600.0).abs() < 0.5, "row starts at y = 600: {:?}", line.rect);
        assert!(line.rect[3] <= 800.0 - 36.0 + 0.5, "row stays within the top margin: {:?}", line.rect);
    }
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.len(), 1, "wrapped turned rows stay one paragraph: {blocks:?}");
    assert_eq!(blocks[0].text, draft);
    assert_eq!(layout.draft_rect, Some(blocks[0].rect));

    // Reading downwards (90° CW) from y = 200: bottom margin is at y = 36.
    let mut doc = text_page("BT /F1 12 Tf 0 -1 1 0 300 200 Tm (A single turned row) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &text::BlockStyle::default()).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert!(lines.len() > 1, "expected 90° CW row to wrap at the bottom margin: {lines:?}");
    for line in &lines {
        assert!((line.rect[3] - 200.0).abs() < 0.5, "row starts at y = 200: {:?}", line.rect);
        assert!(line.rect[1] >= 36.0 - 0.5, "row stays within the bottom margin: {:?}", line.rect);
    }

    // Reading upside down (180°) from x = 200: left margin is at x = 36.
    let mut doc = text_page("BT /F1 12 Tf -1 0 0 -1 200 600 Tm (A single turned row) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &text::BlockStyle::default()).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert!(lines.len() > 1, "expected 180° row to wrap at the left margin: {lines:?}");
    for line in &lines {
        assert!((line.rect[2] - 200.0).abs() < 0.5, "row starts at x = 200: {:?}", line.rect);
        assert!(line.rect[0] >= 36.0 - 0.5, "row stays within the left margin: {:?}", line.rect);
    }

    // Slanted (45°) from (400, 600): baseline stops where it first reaches a 36 pt margin.
    let (sin, cos) = 45.0_f64.to_radians().sin_cos();
    let mut doc = text_page(&format!("BT /F1 12 Tf {cos:.6} {sin:.6} {:.6} {cos:.6} 400 600 Tm (A single turned row) Tj ET", -sin));
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &text::BlockStyle::default()).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert!(lines.len() > 1, "expected 45° slanted row to wrap at the margin: {lines:?}");
    for line in &lines {
        assert!(line.rect[2] <= 600.0 && line.rect[3] <= 800.0, "slanted row stays on the page: {:?}", line.rect);
    }

    // Moving a turned single row with `offset` reduces the room left before the margin.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 500 Tm (A single turned row) Tj ET");
    let moved = text::BlockStyle { offset: Some([0.0, 150.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &moved).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert!(lines.len() > 1, "{lines:?}");
    for line in &lines {
        assert!((line.rect[1] - 650.0).abs() < 0.5, "moved row starts at y = 650: {:?}", line.rect);
        assert!(line.rect[3] <= 800.0 - 36.0 + 0.5, "moved row stops at the top margin: {:?}", line.rect);
    }

    // A turned row that already starts inside the 36 pt margin keeps its own width instead of collapsing.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 770 Tm (Edge label) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("Edge label updated"), &text::BlockStyle::default()).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert!(lines.len() >= 2, "{lines:?}");
}

/// An added item is written out as a specific standard-14 face (`/Times-Italic`, `/Helvetica-Bold`
/// …), so it has to be *measured* with that same face. Measuring the upright face while writing
/// the italic one mis-sizes every box: Times-Italic `A` is 611 against Times-Roman's 722.
#[test]
fn an_items_measured_face_is_the_face_it_is_written_as() {
    for family in [Family::Helvetica, Family::Times, Family::Courier] {
        for bold in [false, true] {
            for italic in [false, true] {
                let written = family.base_font(bold, italic);
                let measured = family.std14(bold, italic);
                assert_eq!(written, measured.base_font(), "{family:?} bold={bold} italic={italic}");
                // ... and the public measure reaches that face, rather than an upright stand-in.
                assert_eq!(family.width("Anna", 12.0, bold, italic), measured.text_width("Anna", 12.0), "{family:?} bold={bold} italic={italic}");
            }
        }
    }
    // The faces really do differ, so the assertions above are not all comparing the same number.
    let times = |italic| Family::Times.width("Anna", 12.0, false, italic);
    assert_ne!(times(false), times(true), "Times-Italic is not Times-Roman");
    // Courier is monospaced in every style: equal widths here are correct, not a collapsed face.
    let courier = |italic| Family::Courier.width("Anna", 12.0, false, italic);
    assert_eq!(courier(false), courier(true));
}

#[test]
fn page_marks_and_added_content_preserve_fractional_user_unit_matrices() {
    for rotation in [0, 90] {
        let mut doc = fixture();
        let page = pdfcraft_model::pages(&doc)[2].obj;
        doc.update_dict(page, |d| {
            let bounds = Object::Array([0.0, 0.0, 0.1, 0.1].into_iter().map(Object::Real).collect());
            d.set(b"MediaBox".to_vec(), bounds.clone());
            d.set(b"CropBox".to_vec(), bounds);
            d.set(b"UserUnit".to_vec(), Object::Int(4096));
            d.set(b"Rotate".to_vec(), Object::Int(rotation));
        })
        .unwrap();
        let mut hf = HeaderFooter::default();
        hf.text[0] = "Matrix header".into();
        add_header_footer(&mut doc, &[2], &hf, false, &cx()).unwrap();
        let text = AddedText { rect: [72.0, 100.0, 200.0, 144.0], text: "Matrix text".into(), size: 12.0, ..AddedText::default() };
        add_content(&mut doc, 2, &Content::Text(text)).unwrap();
        let saved = reopen(&doc);
        let contents = streams(&saved, 2);
        for marker in ["/PCMark /HeaderFooter", "(Matrix text) Tj"] {
            let matching: Vec<_> = contents.iter().filter(|s| s.contains(marker)).collect();
            assert_eq!(matching.len(), 1, "{contents:?}");
            let stream = matching[0];
            let parsed = pdfcraft_content::parse(stream.as_bytes());
            assert_eq!(parsed.skipped, 0);
            let cm = parsed.ops.iter().find(|op| op.is("cm")).unwrap();
            assert_eq!(cm.operands.len(), 6);
            assert!(!stream[cm.span.clone()].contains('e') && !stream[cm.span.clone()].contains('E'), "PDF numbers cannot use exponents");
            let matrix = cm.nums::<6>().unwrap();
            let expected = if rotation == 0 {
                [0.000244140625, 0.0, 0.0, 0.000244140625, 0.0, 0.0]
            } else {
                [0.0, 0.000244140625, -0.000244140625, 0.0, 0.1, 0.0]
            };
            assert_eq!(matrix, expected, "rotation={rotation}, stream={stream}");
            // A fixed physical display point must remain at its intended raw page position.
            let raw = [matrix[0] * 72.0 + matrix[2] * 144.0 + matrix[4], matrix[1] * 72.0 + matrix[3] * 144.0 + matrix[5]];
            let expected_raw = if rotation == 0 { [0.017578125, 0.03515625] } else { [0.06484375, 0.017578125] };
            for (actual, expected) in raw.into_iter().zip(expected_raw) {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn rows_the_edit_does_not_reach_keep_their_source_operators() {
    // Word gaps in TJ, as justified text from a layout program has; the text state alone
    // can't reproduce them, so a rewritten row would lose them.
    let rows = [
        "[(justified) -400 ( text) -400 ( uses) -400 ( wide) -400 ( gaps)] TJ",
        "[(between) -400 ( every) -400 ( single) -400 ( word) -400 ( here)] TJ",
        "(last line.) Tj",
    ];
    let content = format!("BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm {} T* {} T* {} ET", rows[0], rows[1], rows[2]);
    let row_texts = |doc: &Document| -> Vec<(String, [f64; 2])> {
        text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.clone(), l.baseline())).collect()
    };
    let source = row_texts(&text_page(&content));
    assert_eq!(source.len(), 3);
    for (row, from, to) in [(2, "last line.", "last line!"), (0, "justified", "justify"), (1, "single", "lone")] {
        let mut doc = text_page(&content);
        let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.contains("justified")).unwrap();
        let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.replacen(from, to, 1);
        text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        let doc = reopen(&doc);
        let bytes = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
        let after = row_texts(&doc);
        assert_eq!(after.len(), 3, "{to}: {after:?}");
        for (i, ((text, at), (old, was))) in after.iter().zip(&source).enumerate() {
            assert!((at[0] - was[0]).abs() < 0.01 && (at[1] - was[1]).abs() < 0.01, "{to}: row {i} stays at {was:?}, not {at:?}");
            if i == row {
                assert_eq!(*text, old.replacen(from, to, 1), "{to}: the edited row");
                assert!(!bytes.contains(rows[i]), "{to}: the edited row is rewritten");
            } else {
                assert_eq!(text, old, "{to}: row {i}");
                assert_eq!(bytes.matches(rows[i]).count(), 1, "{to}: row {i} keeps its own operator, kerning included:\n{bytes}");
            }
        }
    }
    // Colouring a selection in the last row leaves the rows above it as they were.
    let mut doc = text_page(&content);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.contains("justified")).unwrap();
    let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.clone();
    let at = draft.find("line").unwrap();
    let range = text::RangeStyle {
        range: at..at + 4,
        family: None,
        resource: None,
        size: None,
        color: Some([1.0, 0.0, 0.0]),
        underline: None,
        char_spacing: None,
        scale: None,
    };
    text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle { ranges: vec![range], ..Default::default() }).unwrap();
    let bytes = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(rows[..2].iter().all(|row| bytes.matches(row).count() == 1) && !bytes.contains(rows[2]), "{bytes}");
    assert!(bytes.contains("1 0 0 rg"), "{bytes}");
    // The coloured word is its own run now, so only the rows above compare whole.
    assert_eq!(row_texts(&reopen(&doc))[..2], source[..2]);
}

#[test]
fn kept_rows_stay_in_place_when_rows_wrap_or_move_with_quote_operators() {
    let lines_of = |doc: &Document| -> Vec<(String, [f64; 2])> {
        text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.clone(), l.baseline())).collect()
    };
    let edit = |content: &str, from: &str, to: &str| {
        let mut doc = text_page(content);
        let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.editor_text.contains(from)).unwrap();
        let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.replacen(from, to, 1);
        text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        reopen(&doc)
    };
    // `'` moves to the next line as it shows: a row after a rewritten `'` row can't stay as it is.
    for (quoted, from, to) in [
        ("BT /F1 12 Tf 14 TL 72 700 Td (first row here) Tj (second row here) ' (third row here) ' ET", "second", "2nd"),
        ("BT /F1 12 Tf 14 TL 72 700 Td (first row here) Tj (second row here) ' (third row here) ' ET", "third", "3rd"),
        // Here the first row's own `'` makes the step down that the rows after it build on.
        ("BT /F1 12 Tf 14 TL 72 714 Td (first row here) ' (second row here) ' (third row here) ' ET", "first", "1st"),
    ] {
        let source = lines_of(&text_page(quoted));
        let after = lines_of(&edit(quoted, from, to));
        assert_eq!(after.len(), 3, "{to}: {after:?}");
        for ((text, at), (old, was)) in after.iter().zip(&source) {
            assert_eq!(*text, old.replacen(from, to, 1));
            assert!((at[0] - was[0]).abs() < 0.01 && (at[1] - was[1]).abs() < 0.01, "{to}: {text} stays at {was:?}, not {at:?}");
        }
    }
    // A row that grows past the paragraph's width wraps and pushes the rows below it down;
    // those are rewritten in their new places, never left behind as well.
    let rows = "BT /F1 12 Tf 14.4 TL 72 700 Td (one two three four) Tj T* (five six seven eight) Tj T* (nine ten eleven twelve) Tj ET";
    let doc = edit(rows, "two", "two and a great many more words");
    let text: String = lines_of(&doc).into_iter().map(|(t, _)| t + " ").collect();
    for word in ["one", "five", "nine", "twelve"] {
        assert_eq!(text.matches(word).count(), 1, "{word} appears once: {text}");
    }
    assert!(lines_of(&doc).len() > 3, "the edited row wrapped: {text}");
}

/// A page whose `/F1` is Inter embedded as a simple WinAnsi TrueType font, under `base_font`.
fn embedded_program_page(content: &str, base_font: &str) -> Document {
    let mut doc = text_page(content);
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(base_font));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("TrueType"));
    font.set(b"BaseFont".to_vec(), Object::name(base_font));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    font.set(b"FirstChar".to_vec(), Object::Int(32));
    font.set(b"LastChar".to_vec(), Object::Int(126));
    font.set(b"Widths".to_vec(), Object::Array((32..=126).map(|_| Object::Int(560)).collect()));
    font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    doc.set(pdfcraft_cos::ObjRef::new(5, 0), Object::Dict(font));
    reopen(&doc)
}

/// The font programs and subtypes of the page's font resources.
fn page_font_programs(doc: &Document) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()).unwrap_or_default();
    fonts
        .iter()
        .filter_map(|(_, f)| doc.resolve(f).as_dict().cloned())
        .map(|f| (f.name(b"Subtype").unwrap_or_default().to_vec(), pdfcraft_fonts::pdf::embedded_sfnt(doc, &f)))
        .collect()
}

#[test]
fn a_new_character_comes_from_the_source_fonts_own_embedded_program() {
    let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    for (content, from, to) in [
        // One row (the single-style path) and a row of a two-row paragraph (the row editor's path).
        ("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET", "text", "tőxt"),
        ("BT /F1 12 Tf 14 TL 72 700 Td (First row) Tj T* (Second row) Tj ET", "Second", "Sőcond"),
    ] {
        let mut doc = embedded_program_page(content, "Inter-Regular");
        let block = text::text_blocks(&doc, 0).unwrap()[0].clone();
        let draft = block.editor_text.replacen(from, to, 1);
        let edit = text::rewrite_block(&mut doc, 0, 0, Some(&draft), &text::BlockStyle::default()).unwrap();
        assert_eq!(edit.substituted, None, "{to}: no other family stands in");
        let doc = reopen(&doc);
        // The new character is its own run in the other resource, read back into the same word.
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, draft, "{to}");
        let fonts = page_font_programs(&doc);
        assert!(fonts.iter().all(|(subtype, _)| subtype != b"Type3"), "{to}: no fallback glyphs");
        assert!(
            fonts.iter().any(|(subtype, program)| subtype == b"Type0" && program.as_deref() == Some(inter.as_slice())),
            "{to}: the source program draws it"
        );
    }
}

#[test]
fn a_shaped_run_is_one_text_object_with_each_glyph_where_shaping_put_it() {
    let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET");
    let resource = text::register_text_font(&mut doc, 0, inter).unwrap();
    // Marks shaped beside their letters (q́, x̣) and one stacked above another (ñ̄).
    let typed = "q\u{301}x\u{323} n\u{303}\u{304} Cafe\u{301}";
    let style = text::BlockStyle { width: Some(400.0), resource: Some(resource), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some(typed), &style).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].editor_text, typed);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let start = ops.iter().position(|o| o.is("BDC")).unwrap();
    let end = start + ops[start..].iter().position(|o| o.is("EMC")).unwrap();
    let run = &ops[start..end];
    let count = |name: &str| run.iter().filter(|o| o.is(name)).count();
    assert_eq!((count("BT"), count("Tm"), count("Tj")), (1, 1, 0), "{run:?}");
    // Each glyph's origin, read back with the text-space displacements of ISO 32000-1 §9.4.4.
    let font = run.iter().find(|o| o.is("Tf")).unwrap();
    let (resource, size) = (font.operands[0].as_name().unwrap().to_vec(), font.operands[1].as_f64().unwrap());
    let page = pdfcraft_model::pages(&doc).remove(0);
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, doc.resolve(fonts.get(&resource).unwrap()).as_dict().unwrap());
    let (mut x, mut rise, mut drawn) = (0.0, 0.0, Vec::new());
    for op in run {
        match op.op.as_slice() {
            b"Ts" => rise = op.operands[0].as_f64().unwrap(),
            b"Tc" | b"Tw" => assert_eq!(op.operands[0].as_f64(), Some(0.0), "{run:?}"),
            b"Tz" => assert_eq!(op.operands[0].as_f64(), Some(100.0), "{run:?}"),
            b"TJ" => {
                for item in op.operands[0].as_array().unwrap() {
                    match item.as_string() {
                        Some(s) => {
                            for code in s.bytes.chunks(2) {
                                drawn.push((x, rise));
                                x += metrics.width(u32::from(u16::from_be_bytes([code[0], code[1]]))) * size;
                            }
                        }
                        None => x -= item.as_f64().unwrap() / 1000.0 * size,
                    }
                }
            }
            _ => {}
        }
    }
    let shaped = pdfcraft_fonts::source_shaping::shape(inter, typed).unwrap();
    assert_eq!(drawn.len(), shaped.glyphs.len());
    assert!(shaped.glyphs.iter().any(|g| g.position[1] != 0.0), "a mark is raised");
    for (glyph, (x, y)) in shaped.glyphs.iter().zip(&drawn) {
        assert!((glyph.position[0] * size - x).abs() < 1e-3 && (glyph.position[1] * size - y).abs() < 1e-3, "{glyph:?} at {x}, {y}");
    }
    assert_eq!(rise, 0.0, "the rise is reset after the run");
}

#[test]
fn a_subset_program_is_not_trusted_for_new_characters_and_the_error_names_the_character() {
    let mut doc = embedded_program_page("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET", "ABCDEF+Inter-Regular");
    // The subset's own program is not trusted for a character: a fallback face draws it through a
    // generated font instead (the subset may map a code whose outline was dropped). The Japanese
    // sans face leads the fallbacks when built with craft-fonts; the bundled Inter face otherwise.
    let edit = text::rewrite_block(&mut doc, 0, 0, Some("Stable tőxt"), &text::BlockStyle::default()).unwrap();
    let face = pdfcraft_fonts::document_japanese_font_for_style(false, false).map_or("Inter", |f| f.family);
    assert_eq!(edit.substituted, Some(format!("{face} Type3")));
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].editor_text, "Stable tőxt");
    // A character no fallback face has is refused, named, and left as it was.
    let mut doc = embedded_program_page("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET", "ABCDEF+Inter-Regular");
    let before = page_content_bytes(&doc, 0);
    let result = text::rewrite_block(&mut doc, 0, 0, Some("Stable กtext"), &text::BlockStyle::default());
    let Err(EditError::Invalid(msg)) = result else { panic!("expected a clear error, got {result:?}") };
    assert!(msg.contains("ก (U+0E01)") && msg.contains("Load font") && !msg.contains("Japanese"), "{msg}");
    assert_eq!(page_content_bytes(&doc, 0), before);
}

#[test]
fn draft_layout_matches_the_paragraph_an_applied_edit_draws() {
    let content = "BT /F1 12 Tf 14 TL 72 700 Td (A first paragraph of words) Tj T* (with a second row) Tj ET \
                   BT /F1 12 Tf 72 600 Td (Another paragraph below) Tj ET BT /F1 12 Tf 320 700 Td (Side note) Tj ET";
    let doc = text_page(content);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.starts_with("A first")).unwrap();
    let source = text::text_blocks(&doc, 0).unwrap()[block].clone();
    for draft in [
        source.editor_text.replacen("words", "many more words than before", 1),
        source.editor_text.replacen(" of words", "", 1),
        format!("{}\nand a third row", source.editor_text),
    ] {
        let layout = text::text_edit_layout(&doc, 0, block, &draft, &text::BlockStyle::default()).unwrap();
        // Apply the same edit and measure every run that isn't one of the page's other runs.
        let mut applied = doc.clone();
        text::rewrite_block(&mut applied, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        let others: Vec<_> =
            text::text_lines(&doc, 0).unwrap().into_iter().enumerate().filter(|(i, _)| !source.lines.contains(i)).map(|(_, l)| l.rect).collect();
        let drawn = text::text_lines(&applied, 0)
            .unwrap()
            .into_iter()
            .map(|l| l.rect)
            .filter(|r| !others.contains(r))
            .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
        assert_eq!(layout.draft_rect, drawn, "{draft:?}");
        assert_eq!(layout.source_rect, source.rect);
    }
}

#[test]
fn japanese_paragraphs_wrap_between_characters_without_closing_marks_at_a_row_start() {
    if pdfcraft_fonts::document_japanese_font().is_none() {
        eprintln!("skipping the Japanese wrapping checks: built without craft-fonts (set CRAFT_FONTS_DIR)");
        return;
    }
    let text = "日本語の文章は単語の間に空白がないので、どの文字の間でも行を折り返せます。ただし「句読点」や小さい「っ」は行頭に来ません。";
    // One paragraph (the single-style path), and a heading row before it (the row editor's path).
    for draft in [text.to_string(), format!("見出し\n{text}")] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Placeholder text) Tj ET");
        text::rewrite_block(&mut doc, 0, 0, Some(&draft), &text::BlockStyle { width: Some(120.0), ..Default::default() }).unwrap();
        let block = text::text_blocks(&reopen(&doc), 0).unwrap().remove(0);
        let rows: Vec<&str> = block.editor_text.split('\n').collect();
        assert!(rows.len() > 4, "{rows:?}");
        assert_eq!(rows.concat(), draft.replace('\n', ""), "no space is added between the pieces");
        assert!(block.rect[2] - block.rect[0] <= 120.5, "{:?} {rows:?}", block.rect);
        for row in &rows {
            assert!(!row.starts_with(['、', '。', '」', 'っ']) && !row.ends_with('「'), "{row:?} in {rows:?}");
        }
    }
}

#[test]
fn rewritten_paragraph_is_drawn_after_backgrounds_between_its_rows() {
    // Each row has its own background box drawn immediately before its BT..ET, as cards and
    // highlighted headings exported from LibreOffice or browsers do (#2377).
    let card = "0 0.45 0.25 rg 60 696 240 16 re f BT /F1 10 Tf 1 1 1 rg 64 700 Td (SEMINARIO ONLINE) Tj ET \
                0 0.45 0.25 rg 60 668 240 24 re f BT /F1 16 Tf 1 1 1 rg 64 674 Td (Dalla teoria alla pratica:) Tj ET \
                0 0.45 0.25 rg 60 632 240 32 re f BT /F1 24 Tf 1 1 1 rg 64 640 Td (gestire con metodo) Tj ET \
                0 0.45 0.25 rg 60 596 240 32 re f BT /F1 24 Tf 1 1 1 rg 64 604 Td (un progetto complesso) Tj ET";
    let shown_at = |ops: &[pdfcraft_content::Op], needle: &str| {
        ops.iter()
            .position(|op| {
                matches!(op.op.as_slice(), b"Tj" | b"TJ")
                    && op.operands.iter().any(|o| matches!(o, Object::String(s) if String::from_utf8_lossy(&s.bytes).contains(needle)))
            })
            .unwrap()
    };

    // 1. Editing the title rows emits all rewritten rows after all four background fills.
    let mut doc = text_page(card);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 1);
    let edited = "SEMINARIO ONLINE\nDalla teoria alla pratica:\ngestire con criterio\nun progetto nuovo";
    text::rewrite_block(&mut doc, 0, 0, Some(edited), &text::BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, edited);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let fills: Vec<usize> = ops.iter().enumerate().filter_map(|(i, op)| op.is("f").then_some(i)).collect();
    assert_eq!(fills.len(), 4);
    assert!(
        shown_at(&ops, "criterio") > fills[3] && shown_at(&ops, "nuovo") > fills[3],
        "rewritten title rows must be drawn after their row background fills (fills at {fills:?}): {ops:?}"
    );

    // 2. Partial edit with a kept suffix row: rewritten rows 0..=2 are drawn after fills[0..=2]
    // and still before kept row 3, preserving reading order.
    let mut doc = text_page(card);
    let partial = "WORKSHOP ONLINE\nDalla teoria alla pratica:\ngestire con criterio\nun progetto complesso";
    text::rewrite_block(&mut doc, 0, 0, Some(partial), &text::BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, partial);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let fills: Vec<usize> = ops.iter().enumerate().filter_map(|(i, op)| op.is("f").then_some(i)).collect();
    assert!(
        shown_at(&ops, "WORKSHOP") > fills[2]
            && shown_at(&ops, "pratica:") > fills[2]
            && shown_at(&ops, "criterio") > fills[2]
            && shown_at(&ops, "criterio") < shown_at(&ops, "complesso")
            && shown_at(&ops, "complesso") > fills[3],
        "rewritten prefix rows must follow their fills and precede the kept final row"
    );

    // 3. Editing row 2 so that it wraps pushes row 3 down; all rewritten rows must follow fills[3].
    let mut doc = text_page(card);
    let wrapped_edit = "SEMINARIO ONLINE\nDalla teoria alla pratica:\ngestire con criterio e metodo approfondito\nun progetto complesso";
    text::rewrite_block(&mut doc, 0, 0, Some(wrapped_edit), &text::BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let last_fill = ops.iter().rposition(|op| op.is("f")).unwrap();
    assert!(
        shown_at(&ops, "approfondito") > last_fill && shown_at(&ops, "complesso") > last_fill,
        "wrapped rows and pushed-down suffix row must follow all row background fills"
    );

    // 4. Single-style paragraph path across a CTM change between rows, with movement, underline,
    // and a trailing unrelated line sharing the second row's BT..ET.
    let cross_ctm = "0.9 0.9 0.2 rg 60 696 240 18 re f BT /F1 12 Tf 0 0 0 rg 64 700 Td (First highlighted row) Tj ET \
                     q 1 0 0 1 10 12 cm 0.9 0.9 0.2 rg 50 668 240 18 re f \
                     BT /F1 12 Tf 0 0 0 rg 54 672 Td (Second highlighted row) Tj 246 -40 Td (Side footer) Tj ET Q";
    let mut doc = text_page(cross_ctm);
    let para = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.starts_with("First highlighted")).unwrap();
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[para].rows.len(), 2);
    text::rewrite_block(
        &mut doc,
        0,
        para,
        Some("Updated first highlighted row and updated second highlighted row"),
        &text::BlockStyle { offset: Some([6.0, -4.0]), underline: Some(true), ..Default::default() },
    )
    .unwrap();
    let doc = reopen(&doc);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let last_fill = ops.iter().rposition(|op| op.is("f")).unwrap();
    assert!(
        shown_at(&ops, "Updated first") > last_fill && shown_at(&ops, "Side footer") > shown_at(&ops, "Updated first"),
        "cross-CTM single-style rewrite must follow both background fills and precede the shared BT's trailing line"
    );
    let lines = text::text_lines(&doc, 0).unwrap();
    let updated_first = lines.iter().find(|l| l.text.starts_with("Updated first")).unwrap();
    let side_footer = lines.iter().find(|l| l.text == "Side footer").unwrap();
    assert!(
        (updated_first.baseline()[0] - 70.0).abs() < 0.01 && (updated_first.baseline()[1] - 696.0).abs() < 0.01,
        "offset baseline in user space across CTM: {:?}",
        updated_first.baseline()
    );
    assert!(
        (side_footer.baseline()[0] - 310.0).abs() < 0.01 && (side_footer.baseline()[1] - 644.0).abs() < 0.01,
        "trailing line in shared BT keeps its tlm across CTM: {:?}",
        side_footer.baseline()
    );
}

/// Small graphics-state oracle for synthetic streams: every tracked state change must
/// have been restored when a probe XObject would draw. No external assets are involved.
fn probe_states(doc: &Document) -> Vec<std::collections::BTreeMap<Vec<u8>, Vec<Object>>> {
    let bytes = streams(doc, 0);
    let joined = pdfcraft_content::Pieces::join(&bytes);
    let mut state = std::collections::BTreeMap::new();
    let mut stack = Vec::new();
    let mut probes = Vec::new();
    for op in joined.parse().ops {
        match op.op.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => state = stack.pop().expect("balanced graphics state"),
            b"cm" | b"W" | b"W*" | b"Tz" | b"Tc" | b"Tw" | b"Tr" | b"cs" => {
                state.insert(op.op, op.operands);
            }
            b"Do" if op.name(0) == Some(b"PCProbe") => probes.push(state.clone()),
            _ => {}
        }
    }
    probes
}

fn isolation_fixture(parts: &[(&str, &str)]) -> Document {
    let mut doc = fixture();
    let page = page_list(&doc).remove(0);
    let list = parts
        .iter()
        .map(|(tag, bytes)| {
            Object::Ref(doc.add(Object::Stream(Stream::from_raw(if tag.is_empty() { Dict::new() } else { tagged(tag) }, bytes.as_bytes().to_vec()))))
        })
        .collect();
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list))).unwrap();
    doc
}

#[test]
fn new_marks_isolate_state_appended_after_old_wrappers() {
    for dirt in ["0.5 0 0 0.5 0 0 cm", "0 0 200 200 re W n", "50 Tz", "5 Tc 20 Tw", "3 Tr", "/Pattern cs"] {
        for tag in ["", "OCR", "Wrap"] {
            let mut doc = isolation_fixture(&[
                ("Wrap", "q %PdfCraft\n"),
                ("", "2 0 0 2 0 0 cm"),
                ("Wrap", "Q %PdfCraft\n"),
                ("Added", "q /PCProbe Do Q"),
                (tag, dirt),
            ]);
            let before = probe_states(&doc);
            stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
            let after = probe_states(&reopen(&doc));
            assert_eq!(&after[..before.len()], &before, "existing content must not change");
            assert!(after.last().unwrap().is_empty(), "tag={tag}, dirt={dirt}: {after:?}");
        }
    }
}

#[test]
fn new_marks_isolate_dirty_tagged_only_pages() {
    let mut doc = isolation_fixture(&[("OCR", "50 Tz 5 Tc 20 Tw")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    assert!(probe_states(&doc).last().unwrap().is_empty());
}

#[test]
fn new_marks_do_not_accumulate_wrappers_for_self_contained_marks() {
    let mut doc = isolation_fixture(&[("", "2 0 0 2 0 0 cm")]);
    for _ in 0..10 {
        stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    }
    assert_eq!(streams(&doc, 0).len(), 13);
    assert!(probe_states(&doc).iter().all(|s| s.is_empty()));
}

#[test]
fn new_marks_close_wrapper_after_an_unterminated_comment() {
    let mut doc = isolation_fixture(&[("", "50 Tz % no final newline")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    // Check literal concatenation too: a separator must exist in the saved stream itself,
    // not just in the inspection parser, or the comment can swallow the closing Q.
    let bytes = streams(&doc, 0).concat();
    let parsed = pdfcraft_content::parse(bytes.as_bytes());
    assert_eq!(parsed.ops.iter().filter(|op| op.is("Q")).count(), 2);
    assert!(probe_states(&doc).last().unwrap().is_empty());
}

#[test]
fn new_marks_wrap_unbalanced_graphics_state_whole_without_changing_it() {
    // Content that can't be isolated precisely is wrapped whole, as before #3084, instead of
    // refusing: redaction stamps its boxes after removing the text, so it can't back out.
    for (data, probe_clean) in [("q 50 Tz", true), ("50 Tz q", true), ("q q Q", true), ("Q 50 Tz", false), ("q Q Q 50 Tz", false)] {
        let mut doc = isolation_fixture(&[("Wrap", data)]);
        let page = page_list(&doc).remove(0);
        place_tagged(&mut doc, &page, "Probe", b"q /PCProbe Do Q".to_vec(), false).unwrap();
        let after = streams(&doc, 0);
        assert_eq!(after.len(), 4, "{data}: {after:?}");
        assert_eq!((after[0].as_str(), after[1].as_str()), ("q %PdfCraft\n", data), "{data}");
        // A `q` the content leaves open is closed after it, so the mark starts clean. A stray
        // `Q` pops the wrapper itself: nothing placed after it can undo that.
        let probe = probe_states_lenient(&doc);
        assert_eq!(probe.last().unwrap().is_empty(), probe_clean, "{data}: {after:?}");
    }
}

/// [`probe_states`] for unbalanced content: a `Q` with nothing saved leaves the state as is,
/// as renderers do.
fn probe_states_lenient(doc: &Document) -> Vec<std::collections::BTreeMap<Vec<u8>, Vec<Object>>> {
    let joined = pdfcraft_content::Pieces::join(&streams(doc, 0));
    let mut state = std::collections::BTreeMap::new();
    let mut stack = Vec::new();
    let mut probes = Vec::new();
    for op in joined.parse().ops {
        match op.op.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => state = stack.pop().unwrap_or(state),
            b"cm" | b"W" | b"W*" | b"Tz" | b"Tc" | b"Tw" | b"Tr" | b"cs" => {
                state.insert(op.op, op.operands);
            }
            b"Do" if op.name(0) == Some(b"PCProbe") => probes.push(state.clone()),
            _ => {}
        }
    }
    probes
}

#[test]
fn new_marks_preserve_cross_stream_operands_and_q_pairs() {
    let mut doc = isolation_fixture(&[("", "q 2 0 0 2"), ("", "0 0 cm Q 50"), ("", "Tz")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    assert!(probe_states(&doc).last().unwrap().is_empty());
}

#[test]
fn removing_marks_keeps_wrappers_needed_by_later_external_content() {
    let mut doc = isolation_fixture(&[
        ("Wrap", "q %PdfCraft\n"),
        ("", "2 0 0 2 0 0 cm"),
        ("Wrap", "Q %PdfCraft\n"),
        ("HeaderFooter", "q /PCProbe Do Q"),
        ("", "/PCProbe Do"),
    ]);
    remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap();
    assert_eq!(probe_states(&doc).len(), 1);
    assert!(probe_states(&doc)[0].is_empty(), "the external content still needs the original reset");
}

#[test]
fn new_marks_wrap_unfinished_text_paths_and_operands_whole() {
    for data in ["BT", "ET", "BT BT ET", "0 0 10 10 re", "q 0 0 m Q", "50"] {
        let mut doc = isolation_fixture(&[("Background", "q Q"), ("", data)]);
        let page = page_list(&doc).remove(0);
        place_tagged(&mut doc, &page, "Probe", b"q /PCProbe Do Q".to_vec(), false).unwrap();
        // The behind-mark stays outside the wrapper and the content itself is untouched.
        assert_eq!(streams(&doc, 0), ["q Q", "q %PdfCraft\n", data, "\nQ %PdfCraft\n", "q /PCProbe Do Q"], "{data}");
    }
}

#[test]
fn new_marks_do_not_reveal_cross_stream_comments() {
    let mut doc = isolation_fixture(&[("", "q 50 Tz % comment continues"), ("", "Q 20 Tc\nQ 5 Tw")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    assert!(probe_states(&doc).last().unwrap().is_empty());
    let joined = pdfcraft_content::Pieces::join(&streams(&doc, 0));
    assert!(!joined.parse().ops.iter().any(|op| op.is("Tc")), "commented content must remain inert");
}

#[test]
fn new_marks_follow_clean_streams_ending_in_comments() {
    let mut doc = isolation_fixture(&[("Added", "q 50 Tz Q % no newline")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    assert_eq!(probe_states(&doc).len(), 1, "the new mark must not be swallowed by a comment");
    assert!(probe_states(&doc)[0].is_empty());
}

#[test]
fn removing_marks_does_not_strip_arbitrary_wrap_tagged_content() {
    let mut doc = isolation_fixture(&[("Wrap", "q /PCProbe Do Q"), ("HeaderFooter", "q Q")]);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(probe_states(&doc).len(), 1);
}

#[test]
fn removing_marks_strips_nested_trailing_wrapper_pairs() {
    let mut doc = isolation_fixture(&[
        ("Wrap", "q %PdfCraft\n"),
        ("Wrap", "q %PdfCraft\n"),
        ("", "2 0 0 2 0 0 cm"),
        ("Wrap", "Q %PdfCraft\n"),
        ("Wrap", "Q %PdfCraft\n"),
        ("HeaderFooter", "q Q"),
    ]);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(streams(&doc, 0), ["2 0 0 2 0 0 cm"]);
}

#[test]
fn updating_marks_on_unreadable_content_does_not_nest_wrappers() {
    for data in ["q 50 Tz", "Q 50 Tz", "BT"] {
        let mut doc = isolation_fixture(&[("", data)]);
        let page = page_list(&doc).remove(0);
        for _ in 0..3 {
            place_tagged(&mut doc, &page, MarkKind::HeaderFooter.tag(), b"q /PCProbe Do Q".to_vec(), false).unwrap();
            assert_eq!(streams(&doc, 0).len(), 4, "{data}: {:?}", streams(&doc, 0));
            assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
            assert_eq!(streams(&doc, 0), [data], "{data}: the original list comes back");
        }
    }
}

#[test]
fn removing_marks_strips_wrappers_that_closed_open_state() {
    let mut doc = isolation_fixture(&[("", "q 2 0 0 2 0 0 cm")]);
    stamp(&mut doc, 0, "Probe", b"q /PCProbe Do Q\n".to_vec()).unwrap();
    assert!(probe_states(&doc).last().unwrap().is_empty());
    // Now balanced, the page reads cleanly: the close (with its extra `Q`) still pairs.
    let page = page_list(&doc).remove(0);
    place_tagged(&mut doc, &page, MarkKind::Watermark.tag(), b"q /PCProbe Do Q\n".to_vec(), false).unwrap();
    assert_eq!(streams(&doc, 0).len(), 5, "no second wrapper: {:?}", streams(&doc, 0));
    let mut doc = isolation_fixture(&[("", "q 2 0 0 2 0 0 cm")]);
    let page = page_list(&doc).remove(0);
    place_tagged(&mut doc, &page, MarkKind::Watermark.tag(), b"q /PCProbe Do Q\n".to_vec(), false).unwrap();
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Watermark).unwrap(), 1);
    assert_eq!(streams(&doc, 0), ["q 2 0 0 2 0 0 cm"]);
}

/// #3087: flattening comments leaves alone what isn't a comment: links, and the non-markup
/// multimedia and print-production annotations (3D, Screen, Movie, RichMedia, PrinterMark,
/// TrapNet, Watermark), which it used to bake into the page and delete with their data.
/// Flattening fields only leaves them alone too; real comments are still flattened.
#[test]
fn flattening_comments_keeps_annotations_that_are_not_comments() {
    let kept = ["Widget", "Link", "3D", "Screen", "Movie", "RichMedia", "PrinterMark", "TrapNet", "Watermark"];
    let flattened = ["Text", "Square", "FileAttachment"];
    let all: Vec<&str> = flattened.iter().chain(kept.iter()).copied().collect();
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 40 20] /Length 26 >>\nstream\n0 0 1 rg 0 0 40 20 re f\nendstream";
    let annots: Vec<String> = all
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!("<< /Type /Annot /Subtype /{s} /Rect [{x} 700 {r} 720] /AP << /N 4 0 R >> /Contents ({s}) >>", x = 20 + 50 * i, r = 60 + 50 * i)
        })
        .collect();
    let refs: Vec<String> = (0..all.len()).map(|i| format!("{} 0 R", 6 + i)).collect();
    let page = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 5 0 R /Annots [{}] >>", refs.join(" "));
    let content = stream("", " ");
    let mut objs: Vec<&str> = vec!["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", &page, form, &content];
    objs.extend(annots.iter().map(String::as_str));
    let left = |doc: &Document| -> Vec<String> {
        let p = &pdfcraft_model::pages(doc)[0];
        let Some(a) = p.dict.get(b"Annots").map(|a| doc.resolve(a)) else { return Vec::new() };
        a.as_array()
            .unwrap()
            .iter()
            .filter_map(|o| doc.resolve(o).as_dict().and_then(|d| d.name(b"Subtype")).map(|s| String::from_utf8_lossy(s).into_owned()))
            .collect()
    };
    let mut doc = build(&objs);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), flattened.len());
    assert_eq!(left(&reopen(&doc)), kept, "only the comments were flattened");
    let mut doc = build(&objs);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "the widget");
    let without_widget: Vec<&str> = all.iter().copied().filter(|s| *s != "Widget").collect();
    assert_eq!(left(&reopen(&doc)), without_widget, "flattening fields touches no annotation that isn't a widget");
}

/// A footnote marker (`Ts` text rise, a smaller `Tf` size) does not split the paragraph around it
/// into separate editable zones (#3252): on `main` this read as three blocks — the text up to the
/// marker, the marker alone, and the text after it — because the row-joining baseline tolerance
/// was scaled by the smaller of the two runs' sizes, far tighter than a typical superscript's rise.
#[test]
fn a_superscript_footnote_marker_stays_in_its_paragraphs_row() {
    let doc = text_page(
        "BT /F1 12 Tf 72 700 Td (and A.) Tj 0 -14 Td (Edward Siecienski) Tj 4 Ts /F1 7.2 Tf (3) Tj 0 Ts /F1 12 Tf (. More words continue the paragraph right after the marker.) Tj ET",
    );
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 4, "the marker is its own line, raised and smaller: {lines:?}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1, "one paragraph, not split at the marker: {blocks:?}");
    assert_eq!(blocks[0].rows.len(), 2, "two visual rows, the second carrying the marker");
    assert_eq!(blocks[0].text, "and A. Edward Siecienski3. More words continue the paragraph right after the marker.", "{:?}", blocks[0]);
}

/// A page whose text is a clipping mask (`mode` Tr) with a fill painted through it.
fn clipping_text_page(mode: i64) -> Document {
    text_page(&format!("q\nBT {mode} Tr /F1 48 Tf 40 60 Td (MASK) Tj ET\n1 0 0 rg 0 0 300 150 re f\nQ\n"))
}

/// Rewriting text that is a clipping path destroyed the artwork it masked: the glyphs went, the
/// mask with them, and the fill behind flooded its own rectangle, in a file that still opened
/// without a warning (#3429). Photoshop exports draw every text object this way. The rewrite is
/// refused instead, as `clear_block` already refuses a preview.
#[test]
fn rewriting_clipping_path_text_is_refused_for_every_clipping_mode() {
    for mode in [4, 5, 6, 7] {
        let before = text::text_lines(&clipping_text_page(mode), 0).unwrap();
        assert_eq!(before.len(), 1, "mode {mode}: the mask is one line");

        let mut doc = clipping_text_page(mode);
        let line = text::replace_line(&mut doc, 0, 0, "EDITED");
        assert!(matches!(&line, Err(EditError::Invalid(m)) if m.contains("clipping path")), "mode {mode}: {line:?}");

        let mut doc = clipping_text_page(mode);
        let block = text::rewrite_block(&mut doc, 0, 0, Some("EDITED"), &BlockStyle::default());
        assert!(matches!(&block, Err(EditError::Invalid(m)) if m.contains("clipping path")), "mode {mode}: {block:?}");

        // Refused means untouched: the mask is still there to shape what is painted through it.
        let after = text::text_lines(&doc, 0).unwrap();
        assert_eq!(after.len(), 1, "mode {mode}");
        assert_eq!(after[0].text, "MASK", "mode {mode}");
    }
}

/// The modes that paint ink are still editable: only 4 to 7 add to the clip path.
#[test]
fn painting_modes_are_still_rewritten() {
    for mode in [0, 1, 2] {
        let mut doc = clipping_text_page(mode);
        assert!(text::replace_line(&mut doc, 0, 0, "EDITED").is_ok(), "mode {mode}");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "EDITED", "mode {mode}");
    }
}

/// A line's runs need not share a rendering mode: nothing in the join requires it, so a line that
/// starts as ink can continue as a mask. The refusal reads the whole line, not the mode its first
/// run set -- otherwise the rewrite passed the guard and took the mask away with the ink.
#[test]
fn a_line_that_starts_as_ink_and_continues_as_a_mask_is_refused() {
    let content = "q\nBT /F1 48 Tf 40 60 Td 0 Tr (TITLE) Tj 7 Tr (MASK) Tj ET\n1 0 0 rg 0 0 300 150 re f\nQ\n";
    let before = text::text_lines(&text_page(content), 0).unwrap();
    assert_eq!(before.len(), 1, "the two runs are one line: {:?}", before.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(before[0].text, "TITLEMASK", "and the mask is the second half of it");

    let mut doc = text_page(content);
    let line = text::replace_line(&mut doc, 0, 0, "EDITED");
    assert!(matches!(&line, Err(EditError::Invalid(m)) if m.contains("clipping path")), "{line:?}");
    let block = text::rewrite_block(&mut doc, 0, 0, Some("EDITED"), &BlockStyle::default());
    assert!(matches!(&block, Err(EditError::Invalid(m)) if m.contains("clipping path")), "{block:?}");
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "TITLEMASK", "refused means untouched");
}

/// Another drawing of the line on the same baseline goes with it, so that the replacement is all
/// that shows -- and that copy can be the mask while the line being replaced is ink. Dropping it
/// destroys the artwork as surely as rewriting the mask itself, so this is refused too.
#[test]
fn a_clipping_copy_on_the_same_baseline_is_refused() {
    let content = "q\nBT 0 Tr /F1 48 Tf 40 60 Td (MASK) Tj ET\nBT 7 Tr /F1 48 Tf 40 60 Td (MASK) Tj ET\n1 0 0 rg 0 0 300 150 re f\nQ\n";
    let before = text::text_lines(&text_page(content), 0).unwrap();
    assert_eq!(before.len(), 2, "two drawings of the same line");

    let mut doc = text_page(content);
    let line = text::replace_line(&mut doc, 0, 0, "EDITED");
    assert!(matches!(&line, Err(EditError::Invalid(m)) if m.contains("clipping path")), "{line:?}");
    let block = text::rewrite_block(&mut doc, 0, 0, Some("EDITED"), &BlockStyle::default());
    assert!(matches!(&block, Err(EditError::Invalid(m)) if m.contains("clipping path")), "{block:?}");
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 2, "refused means untouched");
}

/// #2512: a session's drafts are laid out from the document as it was opened, never from each
/// other, and its commit writes what a one-shot rewrite writes, whatever was laid out before.
#[test]
fn a_text_session_commits_what_a_rewrite_writes_whatever_it_laid_out_before() {
    let page = "BT /F1 12 Tf 14 TL 72 700 Td (The first row of a paragraph) Tj T* (and its second row here.) Tj ET";
    let bold = BlockStyle { bold: Some(true), ..Default::default() };
    let drafts: [(&str, &BlockStyle); 4] = [
        ("The first row of a paragraph\nand its second row here, with more words to wrap it onto a third row", &BlockStyle::default()),
        ("Short", &bold),
        ("The first row\nsecond", &BlockStyle::default()),
        ("The first row of a paragraph\nand its second row here.", &bold),
    ];
    let doc = text_page(page);
    let session = text::TextSession::open(&doc, 0, 0).unwrap();
    assert_eq!(session.source().editor_text, "The first row of a paragraph\nand its second row here.");
    for (text, style) in drafts {
        let fresh = text::text_edit_layout(&text_page(page), 0, 0, text, style).unwrap();
        assert_eq!(session.layout(text, style).unwrap(), fresh, "{text:?} is laid out from the opened page");
    }
    let (text, style) = drafts[0];
    let mut by_session = doc.clone();
    session.commit(&mut by_session, Some(text), style).unwrap();
    let mut by_rewrite = text_page(page);
    text::rewrite_block(&mut by_rewrite, 0, 0, Some(text), style).unwrap();
    assert_eq!(
        pdfcraft_cos::write_full(&by_session, &SaveOptions::default()).unwrap(),
        pdfcraft_cos::write_full(&by_rewrite, &SaveOptions::default()).unwrap(),
        "the commit writes what a rewrite writes"
    );
    // A document changed since the session opened isn't written to.
    let before = pdfcraft_cos::write_full(&by_session, &SaveOptions::default()).unwrap();
    let refused = session.commit(&mut by_session, Some("Again"), &BlockStyle::default());
    assert!(matches!(&refused, Err(EditError::Invalid(m)) if m.contains("changed while it was being edited")), "{refused:?}");
    assert_eq!(pdfcraft_cos::write_full(&by_session, &SaveOptions::default()).unwrap(), before);
    assert!(session.is_current(&doc) && !session.is_current(&by_session));
}

/// A one-page document whose resources have `/GS0` (30 % opacity) and `/L1`, a layer that is OFF.
fn row_state_page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] /Order [5 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [6 0 R] /Count 1 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /ExtGState /ca 0.3 /CA 0.3 >>".into(),
        "<< /Type /OCG /Name (Hidden layer) >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 7 0 R /Resources << /Font << /F1 3 0 R >> /ExtGState << /GS0 4 0 R /GS1 << /Type /ExtGState /ca 0.3 /CA 0.3 /BM /Normal >> /GS2 << /Type /ExtGState /ca 1 /CA 1 >> >> /Properties << /L1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
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

/// Each shown string on page 0 with the state it is drawn in: inside a `/OC` marked-content
/// sequence, after `/GS0 gs`, and its horizontal scale (percent).
fn shown_states(doc: &Document) -> Vec<(String, bool, bool, f64)> {
    let joined = streams(doc, 0).join("\n");
    let (mut oc, mut marks) = (0usize, Vec::new());
    let (mut gs, mut tz, mut stack) = (false, 100.0, Vec::new());
    let mut out = Vec::new();
    for op in pdfcraft_content::parse(joined.as_bytes()).ops {
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let layer = op.name(0) == Some(b"OC".as_slice());
                oc += usize::from(layer);
                marks.push(layer);
            }
            b"EMC" => oc -= usize::from(marks.pop().unwrap()),
            b"q" => stack.push((gs, tz)),
            b"Q" => (gs, tz) = stack.pop().unwrap(),
            b"gs" => gs |= op.name(0) == Some(b"GS0".as_slice()),
            b"Tz" => tz = op.num(0).unwrap(),
            b"Tj" | b"'" => {
                let Some(Object::String(s)) = op.operands.last() else { continue };
                if !s.bytes.is_empty() {
                    out.push((String::from_utf8_lossy(&s.bytes).into_owned(), oc > 0, gs, tz));
                }
            }
            _ => {}
        }
    }
    out
}

/// #3115: a row drawn in a hidden layer, at 30 % opacity or at 80 % horizontal scale keeps that
/// state when another row of the same paragraph is edited; the edited row doesn't take it on.
#[test]
fn editing_a_row_keeps_the_layer_opacity_and_scale_of_the_rows_around_it() {
    let row1 = "BT /F1 12 Tf 72 700 Td (Hello world, this is the first line) Tj ET\n";
    let row2 = "BT /F1 12 Tf 72 686 Td (and the second) Tj ET";
    let pages = [
        (format!("{row1}/OC /L1 BDC {row2} EMC"), (true, false, 100.0)),
        (format!("{row1}q /GS0 gs {row2} Q"), (false, true, 100.0)),
        ("BT /F1 12 Tf 72 700 Td (Hello world, this is the first line) Tj 80 Tz 0 -14 Td (and the second) Tj ET".to_string(), (false, false, 80.0)),
        (format!("{row1}{row2}"), (false, false, 100.0)),
    ];
    for (n, (content, (oc, gs, tz))) in pages.iter().enumerate() {
        let mut doc = row_state_page(content);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let block = blocks.iter().position(|b| b.text.starts_with("Hello")).unwrap();
        let text = blocks[block].text.replacen("Hello", "Jello", 1);
        text::replace_block(&mut doc, 0, block, &text).unwrap();
        let shown = shown_states(&reopen(&doc));
        let find = |needle: &str| shown.iter().filter(|s| s.0.contains(needle)).cloned().collect::<Vec<_>>();
        let first = find("Jello");
        assert_eq!(first.len(), 1, "page {}: {shown:?}", n + 1);
        assert_eq!((first[0].1, first[0].2, first[0].3), (false, false, 100.0), "page {}: the edited row keeps its own state: {shown:?}", n + 1);
        let second = find("and the second");
        assert_eq!(second.len(), 1, "page {}: {shown:?}", n + 1);
        assert_eq!((second[0].1, second[0].2, second[0].3), (*oc, *gs, *tz), "page {}: the other row keeps its state: {shown:?}", n + 1);
    }
    // The control: rows in the same state are still one paragraph.
    let blocks = text::text_blocks(&row_state_page(&pages[3].0), 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "Hello world, this is the first line and the second");
}

/// What keeps rows apart is the graphics state they are drawn in, not the `gs` operators
/// before them: a repeated `gs` without q/Q, or another name with the same settings, is still one
/// paragraph, and a `gs` that changes the opacity or puts it back is not.
#[test]
fn rows_under_repeated_or_equal_gs_states_stay_one_paragraph() {
    let row1 = "BT /F1 12 Tf 72 700 Td (Hello world, this is the first line) Tj ET\n";
    let row2 = "BT /F1 12 Tf 72 686 Td (and the second) Tj ET";
    let paragraphs = |content: String| text::text_blocks(&row_state_page(&content), 0).unwrap().len();
    assert_eq!(paragraphs(format!("/GS0 gs {row1}/GS0 gs {row2}")), 1, "the same name twice");
    assert_eq!(paragraphs(format!("/GS0 gs {row1}/GS0 gs /GS0 gs {row2}")), 1, "the same name three times");
    assert_eq!(paragraphs(format!("/GS0 gs {row1}/GS1 gs {row2}")), 1, "two names, the same settings");
    assert_eq!(paragraphs(format!("/GS2 gs {row1}{row2}")), 1, "settings that are the defaults");
    assert_eq!(paragraphs(format!("{row1}/GS0 gs {row2}")), 2, "another opacity");
    assert_eq!(paragraphs(format!("/GS0 gs {row1}/GS2 gs {row2}")), 2, "the opacity put back");
}

/// A page of three vertical (`Identity-V`) columns at 20 pt: `ABCDEF` at x = 500, the same
/// text 30 pt to its left, and `ABCD` far away. Synthetic font: no font program is needed.
fn vertical_columns_page() -> Document {
    vertical_page(
        "BT /V1 20 Tf 500 720 Td <004100420043004400450046> Tj ET\nBT /V1 20 Tf 470 720 Td <004100420043004400450046> Tj ET\nBT /V1 20 Tf 300 720 Td <0041004200430044> Tj ET",
    )
}

/// A page drawing `content` with the vertical font `/V1` (as in [`vertical_columns_page`]) and
/// Helvetica `/F1`.
fn vertical_page(content: &str) -> Document {
    let mut doc = text_page(content);
    let mut font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("SyntheticCID")),
        (b"Encoding".to_vec(), Object::name("Identity-V")),
    ]);
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <ffff> endcodespacerange 6 beginbfchar <0041> <0041> <0042> <0042> <0043> <0043> <0044> <0044> <0045> <0045> <0046> <0046> endbfchar",
    )));
    font.set(b"ToUnicode".to_vec(), Object::Ref(unicode));
    font.set(
        b"DescendantFonts".to_vec(),
        Object::Array(vec![Object::Dict(Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("CIDFontType2")),
            (b"DW".to_vec(), Object::Int(1000)),
        ]))]),
    );
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let helvetica = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("Helvetica")),
        (b"Encoding".to_vec(), Object::name("WinAnsiEncoding")),
    ]);
    let fonts = Dict::from_iter([(b"V1".to_vec(), Object::Dict(font)), (b"F1".to_vec(), Object::Dict(helvetica))]);
    let resources = Dict::from_iter([(b"Font".to_vec(), Object::Dict(fonts))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    doc
}

#[test]
fn vertical_text_lines_are_columns_and_editing_one_keeps_its_neighbour() {
    let doc = vertical_columns_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    let first = lines.iter().find(|l| l.baseline()[0] == 500.0).unwrap();
    let r = first.rect;
    // Six glyphs, each 20 pt wide and advancing 20 pt down, centred on x = 500.
    assert!((r[0] - 490.0).abs() < 0.5 && (r[2] - 510.0).abs() < 0.5, "{r:?}");
    assert!((r[1] - 600.0).abs() < 0.5 && (r[3] - 720.0).abs() < 0.5, "{r:?}");
    for edit in [0, 1] {
        let mut doc = vertical_columns_page();
        let lines = text::text_lines(&doc, 0).unwrap();
        if edit == 0 {
            let line = lines.iter().position(|l| l.baseline()[0] == 500.0).unwrap();
            text::replace_line(&mut doc, 0, line, "ABCDEA").unwrap();
        } else {
            let blocks = text::text_blocks(&doc, 0).unwrap();
            let block = blocks.iter().position(|b| b.lines.iter().any(|&i| lines[i].baseline()[0] == 500.0)).unwrap();
            assert_eq!(blocks[block].lines.len(), 1, "{blocks:?}");
            text::replace_block(&mut doc, 0, block, "ABCDEA").unwrap();
        }
        let after = text::text_lines(&doc, 0).unwrap();
        let texts: Vec<(f64, &str)> = after.iter().map(|l| (l.baseline()[0], l.text.as_str())).collect();
        assert!(texts.contains(&(500.0, "ABCDEA")), "{texts:?}");
        assert!(texts.contains(&(470.0, "ABCDEF")), "{texts:?}");
        assert!(texts.contains(&(300.0, "ABCD")), "{texts:?}");
    }
}

/// Gives `/V1` an embedded CMap that uses itself: which way its glyphs advance can't be known.
fn make_cmap_cyclic(doc: &mut Document) {
    let cmap = doc.add(Object::Null);
    doc.set(
        cmap,
        Object::Stream(Stream::flate(
            Dict::from_iter([(b"UseCMap".to_vec(), Object::Ref(cmap))]),
            b"1 begincodespacerange <0000> <ffff> endcodespacerange",
        )),
    );
    let p = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(p, |d| {
        if let Some(Object::Dict(res)) = d.get_mut(b"Resources")
            && let Some(Object::Dict(fonts)) = res.get_mut(b"Font")
            && let Some(Object::Dict(font)) = fonts.get_mut(b"V1")
        {
            font.set(b"Encoding".to_vec(), Object::Ref(cmap));
        }
    })
    .unwrap();
}

#[test]
fn text_whose_cmap_writing_mode_cannot_be_read_is_not_edited() {
    let mut doc = vertical_columns_page();
    make_cmap_cyclic(&mut doc);
    let error = text::replace_line(&mut doc, 0, 0, "ABCDEA").unwrap_err();
    assert!(error.to_string().contains("cyclic"), "{error}");
}

#[test]
fn a_cyclic_cmap_falls_back_to_horizontal_and_only_refuses_its_own_lines() {
    let mut doc = vertical_page("BT /V1 20 Tf 100 700 Td <00410042> Tj ET\nBT /F1 12 Tf 100 600 Td (Plain) Tj ET");
    make_cmap_cyclic(&mut doc);
    // The page is still listed: the broken font's text as horizontal, the rest untouched.
    let lines = text::text_lines(&doc, 0).unwrap();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["AB", "Plain"], "{texts:?}");
    assert!(lines[0].rect[2] - lines[0].rect[0] > 30.0 && lines[0].rect[3] - lines[0].rect[1] < 30.0, "{:?}", lines[0].rect);
    assert_eq!(text::reading_blocks(&doc, 0).unwrap().len(), 2);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    // Only the line in the broken font is refused.
    let error = text::replace_line(&mut doc.clone(), 0, 0, "BA").unwrap_err();
    assert!(error.to_string().contains("cyclic"), "{error}");
    let block = text::text_blocks(&doc, 0).unwrap().iter().position(|b| b.text == "AB").unwrap();
    let error = text::replace_block(&mut doc.clone(), 0, block, "BA").unwrap_err();
    assert!(error.to_string().contains("cyclic"), "{error}");
    text::replace_line(&mut doc, 0, 1, "Changed").unwrap();
    let texts: Vec<String> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(texts, ["AB", "Changed"]);
}

#[test]
fn vertical_text_drawn_one_glyph_per_tj_is_one_line() {
    // One text object, a `Tj` per glyph; and a column with a text object per glyph.
    let doc = vertical_page(
        "BT /V1 20 Tf 500 720 Td <0041> Tj <0042> Tj <0043> Tj ET\n\
         BT /V1 20 Tf 1 0 0 1 470 720 Tm <0041> Tj ET\nBT /V1 20 Tf 1 0 0 1 470 700 Tm <0042> Tj ET\nBT /V1 20 Tf 1 0 0 1 470 680 Tm <0043> Tj ET",
    );
    let lines = text::text_lines(&doc, 0).unwrap();
    let columns: Vec<(f64, &str)> = lines.iter().map(|l| (l.baseline()[0], l.text.as_str())).collect();
    assert_eq!(columns, [(500.0, "ABC"), (470.0, "ABC")], "{columns:?}");
    for line in &lines {
        let r = line.rect;
        assert!((r[1] - 660.0).abs() < 0.5 && (r[3] - 720.0).abs() < 0.5, "{r:?}");
    }
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
}

/// Two genuinely stacked rows of ordinary body text must not join into one row just because the
/// first is narrow enough to put the second's start within the #3252 tolerance band: here the
/// second row starts 1.4pt to the left of where the first ends (so the two overlap a little,
/// as compressed text or a tight column can) and 5pt below its baseline — inside the widened 50%
/// tolerance but outside the original 20% one. They stay two rows (and two paragraphs).
#[test]
fn stacked_rows_that_merely_overlap_do_not_join_into_one_row() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (A compressed line of body text here) Tj 190 -5 Td (Second stacked line of body text) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    let gap = lines[1].rect[0] - lines[0].rect[2];
    assert!((-2.4..0.0).contains(&gap), "the rows overlap a little, within the old tolerance band: {gap}");
    let baseline_shift = lines[0].rect[3] - lines[1].rect[3];
    assert!((2.4..6.0).contains(&baseline_shift), "between the old and new vertical tolerance: {baseline_shift}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2, "two separate rows stay two paragraphs, not one: {blocks:?}");
}

/// Two equal-size rows with no horizontal overlap must not join into one row either (#3252):
/// the wider tolerance is for a run distinctly smaller or larger than its neighbour (a
/// super/subscript), not for two ordinary body-text rows that happen to sit close enough
/// vertically — here a 5pt baseline shift, inside the widened 50% tolerance but outside the
/// original 20% one, with the second row starting just past where the first ends.
#[test]
fn equal_size_rows_need_the_tighter_tolerance_even_without_overlap() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Short line one) Tj 75 -5 Td (Second line two here) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].size, lines[1].size, "same size: no super/subscript here");
    let gap = lines[1].rect[0] - lines[0].rect[2];
    assert!(gap >= 0.0, "no horizontal overlap: {gap}");
    let baseline_shift = lines[0].rect[3] - lines[1].rect[3];
    assert!((2.4..6.0).contains(&baseline_shift), "between the old and new vertical tolerance: {baseline_shift}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2, "two separate rows stay two paragraphs, not one: {blocks:?}");
}

/// A `TJ` with a surplus array operand (`[(FIRSTARR)] [(LASTARR)] TJ`) is malformed, but the
/// operator still takes the one immediately before it, as the renderer, MuPDF and pdftotext do
/// (#3569) -- not the first, which this reader took before, showing a word the page never draws.
#[test]
fn a_tj_with_a_surplus_array_reads_the_last_one() {
    let doc = text_page("BT /F1 24 Tf 20 100 Td [(FIRSTARR)] [(LASTARR)] TJ ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "LASTARR", "the operand immediately before TJ, as the page draws: {lines:?}");
}

/// A producer wrote `0.0001 Tc` on one word of a line (Word through macOS Quartz). That spacing
/// can't be seen, so the word stays on its line; spacing that can be seen still keeps runs apart.
#[test]
fn a_word_with_invisible_extra_spacing_stays_on_its_line() {
    let page = |spacing: &str| {
        text_page(&format!(
            "BT /F1 12 Tf 1 0 0 1 72 700 Tm (for company ) Tj ET \
             BT /F1 12 Tf 1 0 0 1 141.36 700 Tm (A) Tj ET \
             BT {spacing} Tc /F1 12 Tf 1 0 0 1 149.364 700 Tm (EROSOL) Tj ET"
        ))
    };
    let blocks = text::text_blocks(&page("0.0001"), 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["for company AEROSOL"]);

    let mut doc = page("0.0001");
    text::rewrite_block(&mut doc, 0, 0, Some("for company AEROSOL LLC"), &BlockStyle::default()).unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["for company AEROSOL LLC"]);
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01, "{:?}", lines[0].rect);

    let blocks = text::text_blocks(&page("0.5"), 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["for company A", "EROSOL"]);
}

/// #3573: past the 256-level marked-content cap every `EMC` must still close its own sequence,
/// so `/ActualText` nested that deep is neither dropped nor applied to too little text.
#[test]
fn actual_text_pairs_past_the_marked_content_cap() {
    let span = "/Span <</ActualText (REPLACED)>> BDC BT /F1 24 Tf 72 700 Td (abc) Tj ET EMC";
    let outer = "/Span <</ActualText (OUTER)>> BDC BT /F1 24 Tf 72 700 Td /Y BMC (abc) Tj EMC (def) Tj ET EMC";
    for (content, expected) in [
        (span.to_string(), "REPLACED"),
        (format!("{}{span}{}", "/X BMC ".repeat(256), " EMC".repeat(256)), "REPLACED"),
        (format!("{}{span}{}", "/X BMC ".repeat(1000), " EMC".repeat(1000)), "REPLACED"),
        (outer.to_string(), "OUTER"),
        (format!("{}{outer}{}", "/X BMC ".repeat(255), " EMC".repeat(255)), "OUTER"),
    ] {
        let doc = text_page(&content);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [expected]);
        let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
        assert_eq!(reading, [expected]);
    }
}

fn page_background_from_content(content: &str, resources: &str) -> Option<[f64; 3]> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [0 0 500 700] /Contents 4 0 R /Resources << {resources} >> >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref_offset = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    page_background(&Document::open(Arc::new(bytes)).unwrap(), 0).unwrap()
}

fn assert_page_background_fill_cases(gs: &str) {
    assert_eq!(page_background_from_content("1 g 0 0 600 800 re f", ""), Some([1.0; 3]), "a producer's white page fill");
    assert_eq!(page_background_from_content("0 0 1 0 k 0 0 500 700 re f", ""), Some([1.0, 1.0, 0.0]), "CMYK, covering the crop box");
    assert_eq!(page_background_from_content("/DeviceRGB cs 1 0 0 sc 0 0 500 700 re f", ""), Some([1.0, 0.0, 0.0]), "cs and sc");
    assert_eq!(page_background_from_content("/Full gs q 2 0 0 2 0 0 cm 0 1 0 rg 0 0 300 400 re f Q", gs), Some([0.0, 1.0, 0.0]), "under a cm");
    assert_eq!(page_background_from_content("1 0 0 rg 0 0 300 800 re f", ""), None, "half the page");
    assert_eq!(page_background_from_content("1 0 0 rg 0 0 300 800 re 300 0 300 800 re f", ""), None, "two halves are not one covering rectangle");
    assert_eq!(
        page_background_from_content("1 0 0 rg 0 0 300 800 re 0 0 600 800 re f", ""),
        Some([1.0, 0.0, 0.0]),
        "one of the rectangles covers it"
    );
    // #2994 review: a second rectangle that cuts a hole: any one under even-odd, one wound the
    // other way under nonzero.
    assert_eq!(page_background_from_content("1 0 0 rg 0 0 600 800 re 100 100 200 200 re f*", ""), None, "even-odd hole");
    assert_eq!(page_background_from_content("1 0 0 rg 0 0 600 800 re 300 100 -200 200 re f", ""), None, "nonzero, reversed inner rectangle");
    assert_eq!(
        page_background_from_content("1 0 0 rg 0 0 600 800 re 100 100 200 200 re f", ""),
        Some([1.0, 0.0, 0.0]),
        "nonzero, same winding: no hole"
    );
    assert_eq!(
        page_background_from_content("1 0 0 rg q -1 0 0 1 600 0 cm 0 0 600 800 re Q 100 100 200 200 re f", ""),
        None,
        "reversed by a mirroring cm"
    );
    assert_eq!(page_background_from_content("/Half gs 1 0 0 rg 0 0 600 800 re f", gs), None, "translucent");
    assert_eq!(page_background_from_content("/Mul gs 1 0 0 rg 0 0 600 800 re f", gs), None, "blended");
    assert_eq!(page_background_from_content("0 0 100 100 re W n 1 0 0 rg 0 0 600 800 re f", ""), None, "clipped to a corner");
    assert_eq!(page_background_from_content("0 0 600 800 re W n 1 0 0 rg 0 0 600 800 re f", ""), Some([1.0, 0.0, 0.0]), "a page-sized clip");
    assert_eq!(page_background_from_content("/Pattern cs /P1 scn 0 0 600 800 re f", ""), None, "a pattern");
    assert_eq!(page_background_from_content("BT ET 0 0 600 800 re S 1 0 0 rg 0 0 600 800 re f", ""), None, "a stroke first");
    assert_eq!(page_background_from_content("0 0 m 600 0 l 600 800 l 0 800 l h f", ""), None, "a path that isn't a rectangle");
    assert_eq!(
        page_background_from_content("1 0 0 rg 0 0 600 800 re 100 100 m 200 100 l 200 200 l h f*", ""),
        None,
        "a page fill with a hole cut in it"
    );
}

fn assert_page_background_transform_and_clip_cases() {
    // #2994 review: a rectangle turned 45° about the crop box's centre covers it only if the crop
    // box's corners are inside it; its bounding box covering the crop box isn't enough. This one
    // reaches 500 from the centre along each axis (its box covers the 500 × 700 crop box) but
    // 354 along the diagonals, short of the corners. One reaching 707 covers them.
    let turned = "0.70710678 0.70710678 -0.70710678 0.70710678 250 350 cm";
    assert_eq!(
        page_background_from_content(&format!("1 0 0 rg {turned} -353.553 -353.553 707.107 707.107 re f"), ""),
        None,
        "a turned fill short of the corners"
    );
    assert_eq!(
        page_background_from_content(&format!("1 0 0 rg {turned} -500 -500 1000 1000 re f"), ""),
        Some([1.0, 0.0, 0.0]),
        "a turned fill over them"
    );
    // The same for a clip.
    assert_eq!(
        page_background_from_content(&format!("{turned} -353.553 -353.553 707.107 707.107 re W n 1 0 0 rg -2000 -2000 4000 4000 re f"), ""),
        None,
        "a turned clip"
    );
    assert_eq!(
        page_background_from_content(&format!("{turned} -500 -500 1000 1000 re W n 1 0 0 rg -2000 -2000 4000 4000 re f"), ""),
        Some([1.0, 0.0, 0.0]),
        "a turned clip over the corners"
    );
    // Clips add up: two that each cover the page leave it covered, one that doesn't among them
    // doesn't.
    assert_eq!(
        page_background_from_content("0 0 600 800 re W n 0 0 550 750 re W n 1 0 0 rg 0 0 600 800 re f", ""),
        Some([1.0, 0.0, 0.0]),
        "two covering clips"
    );
    assert_eq!(
        page_background_from_content("0 0 600 800 re W n 0 0 300 750 re W n 1 0 0 rg 0 0 600 800 re f", ""),
        None,
        "the second clip doesn't cover"
    );
    // Past 64 clipping rectangles the clip counts as unknown.
    let clips = "0 0 600 800 re W n ".repeat(65);
    assert_eq!(page_background_from_content(&format!("{clips}1 0 0 rg 0 0 600 800 re f"), ""), None, "too many clips to follow");
    // A rectangle drawn the other way round (negative width) covers the page all the same.
    assert_eq!(page_background_from_content("1 0 0 rg 600 0 -600 800 re f", ""), Some([1.0, 0.0, 0.0]), "drawn clockwise");
    // The crop box counts as covered within half a percent of its size (2.5 × 3.5 here), the
    // edge of that included.
    assert_eq!(page_background_from_content("1 0 0 rg 2 3 496 694 re f", ""), Some([1.0, 0.0, 0.0]), "within the tolerance");
    assert_eq!(page_background_from_content("1 0 0 rg 2.5 3.5 495 693 re f", ""), Some([1.0, 0.0, 0.0]), "on the tolerance's edge");
    assert_eq!(page_background_from_content("1 0 0 rg 3 4 494 692 re f", ""), None, "past the tolerance");
}

/// #2992: page background detection follows the first full-page fill under text and ignores
/// partial, translucent, clipped, or unsupported fills.
#[test]
fn a_page_painted_first_with_a_full_page_fill_has_that_background() {
    let gs = "/ExtGState << /Half << /ca 0.5 >> /Mul << /BM /Multiply >> /Full << /ca 1 /SMask /None >> >>";
    let mut doc = fixture();
    assert_eq!(page_background(&doc, 0).unwrap(), None, "text first");
    add_background(&mut doc, &[0, 1], &Background { color: [0.0, 0.5, 0.5], opacity: 1.0, ..Background::default() }, false).unwrap();
    let doc = reopen(&doc);
    let teal = page_background(&doc, 0).unwrap().expect("Background's fill");
    assert!(teal.iter().zip([0.0, 0.5, 0.5]).all(|(a, b)| (a - b).abs() < 1e-6), "{teal:?}");
    assert!(page_background(&doc, 1).unwrap().is_some(), "on a turned page too");
    assert_eq!(page_background(&doc, 2).unwrap(), None, "a page with no content");
    assert!(page_background(&doc, 9).is_err());

    assert_page_background_fill_cases(gs);
    assert_page_background_transform_and_clip_cases();
}

#[test]
fn reading_keeps_dotted_leader_entries_whole_and_in_order() {
    let mut content = String::new();
    for (title, page, y) in [("Art.1. Scope", "3", 700), ("Art.2. Eligibility", "7", 680), ("Art.3. Application", "12", 660)] {
        content.push_str(&format!(
            "BT /F1 12 Tf 72 {y} Td ({title}) Tj ET \
             BT /F1 12 Tf 250 {y} Td (....................) Tj ET \
             BT /F1 12 Tf 500 {y} Td ({page}) Tj ET\n"
        ));
    }
    content.push_str("BT /F1 12 Tf 72 640 Td (Appendix) Tj ET BT /F1 12 Tf 250 640 Td (\\205\\205\\205) Tj ET BT /F1 12 Tf 500 640 Td (iv) Tj ET\n");
    content.push_str("BT /F1 12 Tf 72 600 Td (Ordinary body text.) Tj ET BT /F1 12 Tf 72 582 Td (Still the same paragraph.) Tj ET");
    let doc = text_page(&content);
    let blocks: Vec<_> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(
        blocks,
        [
            "Art.1. Scope .................... 3",
            "Art.2. Eligibility .................... 7",
            "Art.3. Application .................... 12",
            "Appendix ……… iv",
            "Ordinary body text. Still the same paragraph.",
        ]
    );
}

/// Text set larger than the widest page PDF allows (14 400 pt) panicked when its paragraph was given
/// a width: the width is clamped between the font size and 14 400 pt, and `f64::clamp` panics when
/// its minimum is the larger. The width is now capped at 14 400 pt whatever the font size: from the
/// file (`Tf`, or a small `Tf` scaled up by `Tm`) or asked for (the automation tools pass it through).
#[test]
fn width_on_a_paragraph_set_larger_than_a_page_is_capped_not_a_panic() {
    let wide = text::BlockStyle { width: Some(200.0), ..Default::default() };
    let huge = text::BlockStyle { size: Some(20_000.0), width: Some(200.0), ..Default::default() };
    for (content, style) in [
        ("BT /F2 20000 Tf 72 700 Td (ab) Tj ET", &wide),
        ("BT /F2 1 Tf 20000 0 0 20000 72 700 Tm (ab) Tj ET", &wide),
        ("BT /F2 12 Tf 72 700 Td (ab) Tj ET", &huge),
    ] {
        let mut doc = text_page(content);
        text::rewrite_block(&mut doc, 0, 0, None, style).unwrap();
    }
}

#[test]
fn added_text_is_wrapped_once_per_write() {
    let mut doc = fixture();
    let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
    let text = AddedText { rect: [72.0, 100.0, 300.0, 700.0], text: lines.join("\n"), size: 10.0, align: Align::Justify, ..AddedText::default() };
    added::WRAP_CALLS.with(|c| c.set(0));
    add_content(&mut doc, 0, &Content::Text(text)).unwrap();
    let calls = added::WRAP_CALLS.with(std::cell::Cell::get);
    assert!(calls <= 4, "the text was wrapped {calls} times for 40 lines");
}

/// Character spacing on added text (#4245) is the PDF's `Tc`, kept in `/PCAdded` so the item
/// reopens with it; an item written without it reads 0.
#[test]
fn added_text_character_spacing_is_drawn_and_reopens() {
    let mut doc = fixture();
    let plain = AddedText { rect: [72.0, 600.0, 400.0, 700.0], text: "Approved".into(), size: 10.0, ..AddedText::default() };
    add_content(&mut doc, 0, &Content::Text(plain.clone())).unwrap();
    add_content(&mut doc, 0, &Content::Text(AddedText { char_spacing: 2.5, ..plain })).unwrap();
    let doc = reopen(&doc);
    let spacings: Vec<f64> = list_added(&doc).iter().map(|a| if let Content::Text(t) = &a.content { t.char_spacing } else { f64::NAN }).collect();
    assert_eq!(spacings, [0.0, 2.5]);
    let page = streams(&doc, 0);
    let tc: Vec<bool> = page.iter().filter(|s| s.contains("(Approved) Tj")).map(|s| s.contains("2.5 Tc")).collect();
    assert_eq!(tc, [false, true], "{page:?}");
}

/// Spacing widens every measure the layout takes: the widest line and wrapping.
#[test]
fn added_text_character_spacing_widens_its_layout() {
    let plain = AddedText { rect: [0.0, 0.0, 1000.0, 100.0], text: "Approved".into(), size: 10.0, ..AddedText::default() };
    let spaced = AddedText { char_spacing: 3.0, ..plain.clone() };
    // After each of the 8 letters, like `Tc`.
    assert!((added::text_width(&spaced) - added::text_width(&plain) - 24.0).abs() < 1e-9);
    let w = added::text_width(&plain);
    let narrow = |t: &AddedText| AddedText { text: "Approved Approved".into(), rect: [0.0, 0.0, 2.0 * w + 5.0, 100.0], ..t.clone() };
    assert_eq!(added::lines(&narrow(&plain)).len(), 1);
    assert_eq!(added::lines(&narrow(&spaced)).len(), 2, "the spaced words no longer fit one line");
}

/// A field tree whose every node lists its child twice is pruned in work linear in its nodes, not
/// in its paths, and a node listing itself is visited once.
#[test]
fn flattening_prunes_a_field_tree_with_repeated_kids_once_per_node() {
    let levels = 16;
    let page = stream("", " ");
    // 5: the flattened widget; 6…: the chain; then the hidden widget at its end, on the page.
    let leaf = 6 + levels;
    let fields = format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R {} 0 R] >> >>", leaf + 2);
    let annots = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R {leaf} 0 R] >>");
    let mut objs: Vec<String> = vec![
        fields,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        annots,
        page,
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (shown) /Rect [10 10 150 40] /F 4 /AP << /N {} 0 R >> >>", leaf + 1),
    ];
    for i in 0..levels {
        let next = 7 + i;
        objs.push(format!("<< /T (n{i}) /Kids [{next} 0 R {next} 0 R] >>"));
    }
    objs.push("<< /Type /Annot /Subtype /Widget /FT /Tx /T (hidden) /Rect [10 90 150 120] /F 2 >>".into());
    objs.push("<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream".into());
    objs.push(format!("<< /T (loop) /Kids [{} 0 R] >>", leaf + 2));
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let mut doc = build(&objs);
    flatten::KEEP_CALLS.with(|c| c.set(0));
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1);
    let calls = flatten::KEEP_CALLS.with(std::cell::Cell::get);
    assert!(calls <= 2 * (levels + 4), "{calls} field-tree visits");
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    let left = doc.resolve(af.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().len();
    assert_eq!(left, 2, "the chain to the hidden widget and the self-listing node stay");
}

/// A field reached first at the end of a long chain has its kids cut off by the depth limit, so
/// they survive there. A shorter path to the same field still looks at its kids: an off-page widget
/// under it is taken out when every field is flattened.
#[test]
fn a_field_reached_again_by_a_shorter_path_is_pruned_there() {
    let chain = 64;
    let page = stream("", " ");
    // 7…: the chain; then the shared field, its off-page widget, and the short path to it.
    let shared = 7 + chain;
    let fields = format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 7 0 R {} 0 R] >> >>", shared + 2);
    let mut objs: Vec<String> = vec![
        fields,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>".into(),
        page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (shown) /Rect [10 10 150 40] /F 4 /AP << /N 6 0 R >> >>".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream".into(),
    ];
    for i in 0..chain {
        objs.push(format!("<< /T (c{i}) /Kids [{} 0 R] >>", 8 + i));
    }
    objs.push(format!("<< /T (shared) /Kids [{} 0 R] >>", shared + 1));
    objs.push("<< /Type /Annot /Subtype /Widget /FT /Tx /T (orphan) /Rect [10 50 150 80] /F 4 >>".into());
    objs.push(format!("<< /T (short) /Kids [{shared} 0 R] >>"));
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let mut doc = build(&objs);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1);
    let shared_ref = pdfcraft_cos::ObjRef::new(u32::try_from(shared).unwrap(), 0);
    let kids = doc.resolve(doc.get(shared_ref).as_dict().unwrap().get(b"Kids").unwrap()).as_array().unwrap().len();
    assert_eq!(kids, 0, "the off-page widget is pruned along the short path");
}

/// A ToUnicode destination is repeated for every glyph that shows its code: a page whose text
/// would run past the extraction budget is refused, not built in memory.
#[test]
fn page_text_past_the_budget_is_refused() {
    let cmap = stream("", &format!("1 beginbfchar <41> <{}> endbfchar", "0041".repeat(256)));
    let page = |shown: &str| {
        let content = stream("", &format!("BT /F1 12 Tf 10 10 Td ({shown}) Tj ET"));
        build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
            &content,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>",
            &cmap,
        ])
    };
    let doc = page(&"A".repeat(100_000));
    assert!(matches!(text_lines(&doc, 0), Err(EditError::Invalid(_))), "25 MB of text from a 100 kB page");
    let doc = page("AA");
    assert_eq!(text_lines(&doc, 0).unwrap()[0].text.len(), 512, "a page within the budget still reads");
}

/// A one-page document from the page's /Contents and /Resources, built in code.
fn page_with(build: impl FnOnce(&mut Document) -> (Object, Dict)) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.add(Object::Null);
    let (contents, resources) = build(&mut doc);
    let page: Dict = [
        (b"Type".to_vec(), Object::Name(b"Page".to_vec())),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::from).to_vec())),
        (b"Contents".to_vec(), contents),
        (b"Resources".to_vec(), Object::Dict(resources)),
    ]
    .into_iter()
    .collect();
    let page = doc.add(page);
    let tree: Dict = [
        (b"Type".to_vec(), Object::Name(b"Pages".to_vec())),
        (b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)])),
        (b"Count".to_vec(), Object::Int(1)),
    ]
    .into_iter()
    .collect();
    doc.set(pages, Object::Dict(tree));
    let catalog: Dict = [(b"Type".to_vec(), Object::Name(b"Catalog".to_vec())), (b"Pages".to_vec(), Object::Ref(pages))].into_iter().collect();
    let catalog = doc.add(catalog);
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(catalog));
    doc
}

#[test]
fn repeated_content_references_share_one_decode_budget() {
    // One 1 MiB stream (a kilobyte on disk) named 300 times by the page's /Contents.
    let doc = page_with(|doc| {
        let spaces = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
        (Object::Array(vec![Object::Ref(spaces); 300]), Dict::new())
    });
    match text_lines(&doc, 0) {
        Ok(lines) => panic!("read {} lines from more content than a page may decode", lines.len()),
        Err(e) => assert!(matches!(e, EditError::Invalid(_)), "{e}"),
    }
}

#[test]
fn reading_images_stops_at_the_page_record_limit() {
    // 20 draws of a form drawing a form 20 times that draws an image 20 times: 8,000 images
    // per level-two form, 160,000 in all, from a few hundred bytes.
    let doc = page_with(|doc| {
        let image: Dict = [
            (b"Subtype".to_vec(), Object::Name(b"Image".to_vec())),
            (b"Width".to_vec(), Object::Int(1)),
            (b"Height".to_vec(), Object::Int(1)),
            (b"ColorSpace".to_vec(), Object::Name(b"DeviceGray".to_vec())),
            (b"BitsPerComponent".to_vec(), Object::Int(8)),
        ]
        .into_iter()
        .collect();
        let mut inner = doc.add(Stream::from_raw(image, vec![0]));
        let draws = "/X Do ".repeat(20);
        let resources = |inner: pdfcraft_cos::ObjRef| -> Dict {
            let xobjects: Dict = [(b"X".to_vec(), Object::Ref(inner))].into_iter().collect();
            [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect()
        };
        for _ in 0..3 {
            let mut d = resources(inner);
            d = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec())), (b"Resources".to_vec(), Object::Dict(d))].into_iter().collect();
            inner = doc.add(Stream::from_raw(d, draws.clone().into_bytes()));
        }
        let page = doc.add(Stream::from_raw(Dict::new(), draws.into_bytes()));
        (Object::Ref(page), resources(inner))
    });
    if let Ok(images) = crate::reading_images(&doc, 0) {
        assert!(images.len() <= crate::images::MAX_PAGE_IMAGES, "{} images", images.len());
    }
}

#[test]
fn reading_spends_one_decode_budget_on_content_and_forms() {
    // 130 MiB of content and a 130 MiB form (a kilobyte each on disk): each fits a page's budget,
    // together they don't, so reading leaves the form out and says so.
    let doc = page_with(|doc| {
        let mut body = b"%".to_vec();
        body.resize(130 << 20, b'x');
        let form: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
        let form = doc.add(Stream::flate(form, &body));
        let mut content = b"/X Do\n%".to_vec();
        content.resize(130 << 20, b'x');
        let content = doc.add(Stream::flate(Dict::new(), &content));
        let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
        (Object::Ref(content), [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect())
    });
    assert!(crate::text::reading_blocks_checked(&doc, 0).unwrap().truncated);
}

/// A page that can't take flattened content on top (marked content nested past the depth budget)
/// refuses the whole flatten: the pages before it keep their comments and content.
#[test]
fn a_flatten_refused_on_a_later_page_changes_nothing() {
    let deep = stream("", &"/Artifact BMC\n".repeat(10_001));
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
        &stream("", " "),
        "<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /F 4 /AP << /N 9 0 R >> >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 7 0 R /Annots [8 0 R] >>",
        &deep,
        "<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /F 4 /AP << /N 9 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 40 40] /Length 0 >>\nstream\n\nendstream",
    ]);
    let first = pdfcraft_cos::ObjRef::new(3, 0);
    let before = (*doc.get(first)).clone();
    assert!(matches!(flatten(&mut doc, &[0, 1], true, false), Err(EditError::Invalid(_))));
    assert_eq!((*doc.get(first)).clone(), before, "the first page keeps its comment and content");
}

/// Existing-row alignment is explicitly refused without mutating the paragraph.
#[test]
fn one_row_of_a_paragraph_takes_its_own_alignment_is_refused_without_changing_the_document() {
    for align in [added::Align::Left, added::Align::Center, added::Align::Right, added::Align::Justify] {
        let mut doc = text_page("BT /F1 10 Tf 12 TL 72 700 Td (First row) Tj T* (Second row) Tj ET");
        let before = page_content_bytes(&doc, 0);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let style = text::BlockStyle { aligns: vec![None, Some(align)], ..Default::default() };
        let error = text::rewrite_block(&mut doc, 0, 0, Some(&blocks[0].editor_text), &style).unwrap_err();
        assert!(error.to_string().contains("selected-row alignment"));
        assert_eq!(page_content_bytes(&doc, 0), before);
        assert_eq!(page_content_bytes(&reopen(&doc), 0), before);
    }
}

/// Refusal preserves all existing rows and their saved content stream.
#[test]
fn rows_without_their_own_alignment_keep_their_operators_is_refused_without_changing_the_document() {
    for align in [added::Align::Left, added::Align::Center, added::Align::Right, added::Align::Justify] {
        let mut doc = text_page("BT /F1 10 Tf 12 TL 72 700 Td (First row) Tj T* (Second row) Tj ET");
        let before = page_content_bytes(&doc, 0);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let style = text::BlockStyle { aligns: vec![None, Some(align)], ..Default::default() };
        let error = text::rewrite_block(&mut doc, 0, 0, Some(&blocks[0].editor_text), &style).unwrap_err();
        assert!(error.to_string().contains("selected-row alignment"));
        assert_eq!(page_content_bytes(&doc, 0), before);
        assert_eq!(page_content_bytes(&reopen(&doc), 0), before);
    }
}

/// Mixed-style rows also refuse unsupported per-row intent before writing.
#[test]
fn one_row_of_a_styled_region_takes_its_own_alignment_is_refused_without_changing_the_document() {
    for align in [added::Align::Left, added::Align::Center, added::Align::Right, added::Align::Justify] {
        let mut doc = mixed_region_page();
        let before = page_content_bytes(&doc, 0);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let style = text::BlockStyle { aligns: vec![None, Some(align)], ..Default::default() };
        let error = text::rewrite_block(&mut doc, 0, 1, Some(&blocks[1].editor_text), &style).unwrap_err();
        assert!(error.to_string().contains("selected-row alignment"));
        assert_eq!(page_content_bytes(&doc, 0), before);
        assert_eq!(page_content_bytes(&reopen(&doc), 0), before);
    }
}

/// Added text: each paragraph is laid out with its own alignment, a justified paragraph's last
/// line isn't stretched, and the alignments survive saving and reopening.
#[test]
fn added_text_paragraphs_keep_their_own_alignments() {
    let mut doc = fixture();
    let text = AddedText {
        rect: [72.0, 600.0, 372.0, 700.0],
        text: "Left line\nRight line\nCentred line".into(),
        size: 12.0,
        aligns: vec![None, Some(Align::Right), Some(Align::Center)],
        ..AddedText::default()
    };
    assert_eq!(text.paragraph_align(1), Align::Right);
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    let doc = reopen(&doc);
    let Content::Text(t) = &list_added(&doc)[0].content else { panic!() };
    assert_eq!(t.aligns, text.aligns, "saved with the item");
    let page = streams(&doc, 0).join("\n");
    let x_of = |s: &str| -> f64 {
        let at = page.find(&format!("({s}) Tj")).unwrap();
        let tm = page[..at].rfind(" Tm").unwrap();
        let nums: Vec<f64> = page[..tm].rsplit("1 0 0 1 ").next().unwrap().split_whitespace().filter_map(|v| v.parse().ok()).collect();
        nums[0]
    };
    let w = |s: &str| Family::Helvetica.width(s, 12.0, false, false);
    assert!((x_of("Left line") - 72.0).abs() < 0.01);
    assert!((x_of("Right line") - (372.0 - w("Right line"))).abs() < 0.01);
    assert!((x_of("Centred line") - (72.0 + (300.0 - w("Centred line")) / 2.0)).abs() < 0.01);
    // A paragraph's own alignment the same as the box's is no entry at all.
    let tidy = paragraph_align::tidy(vec![Some(Align::Left), Some(Align::Right)], Align::Left);
    assert_eq!(tidy, [None, Some(Align::Right)]);
}

/// With character spacing, a paragraph aligned on its own is placed by its spaced width: a
/// right-aligned line still ends at the box's right edge, and a centred one sits in the middle.
#[test]
fn added_text_paragraphs_align_by_their_spaced_width() {
    let mut doc = fixture();
    let text = AddedText {
        rect: [72.0, 600.0, 372.0, 700.0],
        text: "Left line\nRight line\nCentred line".into(),
        size: 12.0,
        aligns: vec![None, Some(Align::Right), Some(Align::Center)],
        char_spacing: 2.0,
        ..AddedText::default()
    };
    add_content(&mut doc, 0, &Content::Text(text)).unwrap();
    let page = streams(&reopen(&doc), 0).join("\n");
    assert!(page.contains("2 Tc"), "{page}");
    let x_of = |s: &str| -> f64 {
        let at = page.find(&format!("({s}) Tj")).unwrap();
        let tm = page[..at].rfind(" Tm").unwrap();
        let nums: Vec<f64> = page[..tm].rsplit("1 0 0 1 ").next().unwrap().split_whitespace().filter_map(|v| v.parse().ok()).collect();
        nums[0]
    };
    let w = |s: &str| Family::Helvetica.width(s, 12.0, false, false) + 2.0 * s.chars().count() as f64;
    assert!((x_of("Left line") - 72.0).abs() < 0.01);
    assert!((x_of("Right line") - (372.0 - w("Right line"))).abs() < 0.01);
    assert!((x_of("Centred line") - (72.0 + (300.0 - w("Centred line")) / 2.0)).abs() < 0.01);
}

/// A one-row title over a body column: an alignment picked for it is measured across the column.
/// Left puts it on the column's left edge, and centring it again leaves it where it is, rather
/// than aligning it in its own box (a no-op) or toward the page's margin (moving it right each
/// time). It is aligned as the editor aligns a selected row, in whichever paragraph holds it.
#[test]
fn a_lone_row_aligns_across_the_column_it_sits_in() {
    let mut doc = text_page(
        "BT /F1 24 Tf 1 0 0 1 250 700 Tm (Title) Tj ET          BT /F1 10 Tf 12 TL 72 650 Td (The body of the page runs across the whole column here.) Tj T* (It has a second row that is a little shorter.) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, "Title");
    let column = [blocks[1].rect[0], blocks[1].rect[2]];
    let title = |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().find(|l| l.text == "Title").unwrap().rect;
    let align = |doc: &mut Document, a: added::Align| {
        let lines = text::text_lines(doc, 0).unwrap();
        let line = lines.iter().position(|l| l.text == "Title").unwrap();
        let blocks = text::text_blocks(doc, 0).unwrap();
        let (index, block) = blocks.iter().enumerate().find(|(_, b)| b.lines.contains(&line)).unwrap();
        // The title is the paragraph's first row.
        let style = text::BlockStyle { align: Some(a), ..Default::default() };
        text::rewrite_block(doc, 0, index, Some(&block.editor_text), &style).unwrap();
    };
    align(&mut doc, added::Align::Center);
    let centred = title(&doc);
    assert!(((centred[0] + centred[2]) / 2.0 - (column[0] + column[1]) / 2.0).abs() < 1.0, "centred in the column: {centred:?} vs {column:?}");
    align(&mut doc, added::Align::Center);
    assert!((title(&doc)[0] - centred[0]).abs() < 0.5, "centring again leaves it where it is: {:?} vs {centred:?}", title(&doc));
    align(&mut doc, added::Align::Left);
    assert!((title(&doc)[0] - column[0]).abs() < 0.5, "left: back on the column's edge, {:?} vs {column:?}", title(&doc));
    align(&mut doc, added::Align::Right);
    assert!((title(&doc)[2] - column[1]).abs() < 1.0, "right: on the column's right edge, {:?} vs {column:?}", title(&doc));
}

/// An edited paragraph's text has a line break between its source rows (`editor_text`), as the
/// paragraph editor sends it. Those are the rows of one paragraph, not paragraph ends: justified
/// in a font of its own (the single-style rewrite), every row but the last still fills the width.
#[test]
fn a_justified_paragraph_from_the_editor_fills_its_rows() {
    let mut doc =
        text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj T* (end) Tj ET");
    let block = text::text_blocks(&doc, 0).unwrap()[0].clone();
    assert_eq!(block.editor_text.matches('\n').count(), 2);
    let style = text::BlockStyle { align: Some(added::Align::Justify), family: Some((Family::Helvetica, true, false)), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some(&block.editor_text), &style).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    let row = |t: &str| lines.iter().find(|l| l.text.starts_with(t)).unwrap().rect;
    let right = row("One")[2];
    assert!((row("eight")[2] - right).abs() < 1.0, "the second row is stretched: {:?} vs {right}", row("eight"));
    assert!(row("end")[2] < right - 20.0, "the last row isn't");
}

/// An alignment for a row the draft doesn't have names no row: a one-row title given one for a
/// second row only stays where it is, not moved to its column's edge. Nor does a row of the draft
/// with no alignment of its own move because another row has one (CodeRabbit on 648dd428).
#[test]
fn a_title_moves_to_its_column_only_for_an_alignment_of_its_own() {
    let page = "BT /F1 24 Tf 1 0 0 1 250 700 Tm (Title) Tj ET          BT /F1 10 Tf 12 TL 72 650 Td (The body of the page runs across the whole column here.) Tj T* (It has a second row that is a little shorter.) Tj ET";
    let title = |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().find(|l| l.text == "Title").unwrap().rect;
    let before = title(&text_page(page));
    for draft in ["Title", "Title\nSub"] {
        let mut doc = text_page(page);
        let style = text::BlockStyle { aligns: vec![None, Some(added::Align::Right)], ..Default::default() };
        assert!(text::rewrite_block(&mut doc, 0, 0, Some(draft), &style).is_err());
        assert!((title(&doc)[0] - before[0]).abs() < 0.5, "{draft:?}: the title stays put, {:?} vs {before:?}", title(&doc));
    }
}

/// A column whose rows are several runs (a second style on the same baseline) reaches the right
/// edge of its whole rows, not of the run at the left edge: a title wider than that first run is
/// still held by the column and aligns to it (CodeRabbit on b9940d7d).
#[test]
fn a_column_is_measured_by_its_whole_rows_not_its_first_runs() {
    let page = "BT /F1 18 Tf 1 0 0 1 100 672 Tm (A longer title) Tj ET                 BT /F1 10 Tf 12 TL 50 610 Td (The first column runs to here.) Tj ET                 BT /F1 11 Tf 1 0 0 rg 1 0 0 1 185 610 Tm (and keeps going on.) Tj ET                 BT /F1 10 Tf 12 TL 50 598 Td (A second row of it.) Tj ET";
    let blocks = text::text_blocks(&text_page(page), 0).unwrap();
    let body = blocks.iter().find(|b| b.text.starts_with("The first")).expect("the body is a block");
    assert!(body.rows.first().is_some_and(|row| row.len() >= 2), "the body's first row is several runs: {:?}", body.rows);
    let lines = text::text_lines(&text_page(page), 0).unwrap();
    let column_right =
        body.rows.iter().filter_map(|row| row.iter().filter_map(|i| lines.get(*i)).map(|l| l.rect[2]).reduce(f64::max)).fold(f64::MIN, f64::max);
    let mut doc = text_page(page);
    let index = blocks.iter().position(|b| b.text == "A longer title").expect("the title is a block of its own");
    let style = text::BlockStyle { align: Some(added::Align::Right), ..Default::default() };
    text::rewrite_block(&mut doc, 0, index, Some("A longer title"), &style).unwrap();
    let title = text::text_lines(&doc, 0).unwrap().into_iter().find(|l| l.text == "A longer title").unwrap().rect;
    assert!((title[2] - column_right).abs() < 1.0, "right: on the whole row's right edge, {title:?} vs {column_right}");
}

/// A row with no column of its own is aligned in its own box, but that box is not the width its new
/// text wraps at: changing the text together with an alignment keeps a single row on one row, as
/// it does without the alignment (CodeRabbit on b9940d7d).
#[test]
fn an_isolated_row_edited_with_an_alignment_does_not_wrap_at_its_old_width() {
    for align in [added::Align::Center, added::Align::Right] {
        let mut doc = text_page("BT /F1 18 Tf 1 0 0 1 100 700 Tm (Title) Tj ET");
        let longer = "A much longer title for the page";
        let style = text::BlockStyle { align: Some(align), ..Default::default() };
        text::rewrite_block(&mut doc, 0, 0, Some(longer), &style).unwrap();
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [longer], "{align:?}: the title stays on one row");
    }
}

/// A heading across the whole page sits closer to the title than the title's own column does: the
/// title is still aligned in its own column, not across the heading's page-wide frame (FredApps's
/// review of 43f848d5). Centre and right both go by the column.
#[test]
fn a_lone_row_aligns_in_its_own_column_not_a_closer_full_width_heading() {
    let page = "BT /F1 12 Tf 1 0 0 1 50 700 Tm (A heading that runs across both columns of the page, well past the first column, to the far margin.) Tj ET                 BT /F1 18 Tf 1 0 0 1 120 672 Tm (Title) Tj ET                 BT /F1 10 Tf 12 TL 50 610 Td (The first column runs to here.) Tj T* (A second row of it.) Tj ET                 BT /F1 10 Tf 12 TL 320 610 Td (The second column has more rows.) Tj T* (One.) Tj T* (Two.) Tj T* (Three.) Tj ET";
    let blocks = text::text_blocks(&text_page(page), 0).unwrap();
    let column = blocks.iter().find(|b| b.text.starts_with("The first")).unwrap().rect;
    let heading = blocks.iter().find(|b| b.text.starts_with("A heading")).unwrap().rect;
    let title_block = blocks.iter().find(|b| b.text == "Title").expect("the title is a block of its own").rect;
    let gap = |other: [f64; 4]| (other[1] - title_block[3]).max(title_block[1] - other[3]).max(0.0);
    assert!(gap(heading) < gap(column), "the heading is the nearer block: {} vs {}", gap(heading), gap(column));
    assert!(heading[2] - heading[0] > 2.0 * (column[2] - column[0]), "the heading is page-wide: {heading:?} vs {column:?}");
    let title_after = |align: added::Align| {
        let mut doc = text_page(page);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let index = blocks.iter().position(|b| b.text == "Title").unwrap();
        let style = text::BlockStyle { align: Some(align), ..Default::default() };
        text::rewrite_block(&mut doc, 0, index, Some("Title"), &style).unwrap();
        text::text_lines(&doc, 0).unwrap().into_iter().find(|l| l.text == "Title").unwrap().rect
    };
    let centred = title_after(added::Align::Center);
    assert!(((centred[0] + centred[2]) / 2.0 - (column[0] + column[2]) / 2.0).abs() < 1.0, "centred in its own column: {centred:?} vs {column:?}");
    let right = title_after(added::Align::Right);
    assert!((right[2] - column[2]).abs() < 1.0, "right: on its own column's right edge, {right:?} vs {column:?}");
}

/// The same on a page of two columns, where the other column has more rows: a title over the
/// first column is aligned across the first column, not its own box.
#[test]
fn a_lone_row_aligns_across_its_own_column_of_two() {
    let mut doc = text_page(
        "BT /F1 18 Tf 1 0 0 1 120 700 Tm (Title) Tj ET \
         BT /F1 10 Tf 12 TL 50 660 Td (The first column runs to here.) Tj T* (A second row of it.) Tj ET \
         BT /F1 10 Tf 12 TL 320 660 Td (The second column has more rows.) Tj T* (One.) Tj T* (Two.) Tj T* (Three.) Tj T* (Four.) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let column = blocks.iter().find(|b| b.text.starts_with("The first")).unwrap().rect;
    let index = blocks.iter().position(|b| b.text == "Title").unwrap();
    let style = text::BlockStyle { align: Some(added::Align::Left), ..Default::default() };
    text::rewrite_block(&mut doc, 0, index, Some("Title"), &style).unwrap();
    let title = text::text_lines(&doc, 0).unwrap().into_iter().find(|l| l.text == "Title").unwrap().rect;
    assert!((title[0] - column[0]).abs() < 0.5, "left: on the first column's edge, {title:?} vs {column:?}");
}

/// A one-row title in two colours is rewritten by the styled-region path, which is given the
/// same column frame: centred twice, it stays centred in the column instead of moving right.
#[test]
fn a_lone_row_of_two_styles_aligns_across_its_column() {
    let mut doc = text_page(
        "BT /F1 18 Tf 1 0 0 1 250 700 Tm (Big ) Tj 1 0 0 rg (Title) Tj ET \
         BT 0 g /F1 10 Tf 12 TL 72 650 Td (The body of the page runs across the whole column here.) Tj T* (It has a second row that is a little shorter.) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, "Big Title");
    let column = [blocks[1].rect[0], blocks[1].rect[2]];
    let title = |doc: &Document| {
        let lines = text::text_lines(doc, 0).unwrap();
        let block = text::text_blocks(doc, 0).unwrap().into_iter().find(|b| b.text == "Big Title").unwrap();
        block.lines.iter().map(|i| lines[*i].rect).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]).unwrap()
    };
    let centre = |doc: &mut Document| {
        let blocks = text::text_blocks(doc, 0).unwrap();
        let (index, block) = blocks.iter().enumerate().find(|(_, b)| b.text == "Big Title").unwrap();
        let style = text::BlockStyle { align: Some(added::Align::Center), ..Default::default() };
        text::rewrite_block(doc, 0, index, Some(&block.editor_text), &style).unwrap();
    };
    centre(&mut doc);
    let centred = title(&doc);
    assert!(((centred[0] + centred[2]) / 2.0 - (column[0] + column[1]) / 2.0).abs() < 1.0, "centred in the column: {centred:?} vs {column:?}");
    centre(&mut doc);
    assert!((title(&doc)[0] - centred[0]).abs() < 0.5, "centring again leaves it where it is: {:?} vs {centred:?}", title(&doc));
}

#[test]
fn a_titles_column_ignores_distant_full_width_headers_and_footers() {
    for (title_x, column_prefix) in [(95, "The first"), (365, "The second")] {
        for mixed in [false, true] {
            for title_y in [710, 580] {
                let title = if mixed { "(Big ) Tj 1 0 0 rg (Title) Tj" } else { "(Title) Tj" };
                let name = if mixed { "Big Title" } else { "Title" };
                let mut doc = text_page(&format!(
                    "BT /F1 10 Tf 12 TL 50 780 Td (A page-wide header extending well beyond either body column on this page and continuing all the way to the right margin.) Tj T* (A repeated page-wide heading across both columns and continuing to the right margin of the page.) Tj ET \
                 BT /F1 18 Tf 1 0 0 1 {title_x} {title_y} Tm {title} ET \
                 BT 0 g /F1 10 Tf 12 TL 50 660 Td (The first column runs to here.) Tj T* (A second row of it.) Tj ET \
                 BT /F1 10 Tf 12 TL 320 660 Td (The second column runs to here.) Tj T* (Another row.) Tj ET \
                 BT /F1 10 Tf 50 40 Td (A page-wide footer extending well beyond either body column on this page and continuing all the way to the right margin.) Tj ET"
                ));
                let rect = |doc: &Document, prefix: &str| {
                    let blocks = text::text_blocks(doc, 0).unwrap();
                    blocks.iter().find(|b| b.text.starts_with(prefix)).unwrap_or_else(|| panic!("missing {prefix:?} in {blocks:?}")).rect
                };
                let column = rect(&doc, column_prefix);
                let header = rect(&doc, "A page-wide header");
                let footer = rect(&doc, "A page-wide footer");
                assert!(header[2] > column[2] + 20.0 && footer[2] > column[2] + 20.0);
                for align in [added::Align::Center, added::Align::Center, added::Align::Right, added::Align::Left] {
                    let blocks = text::text_blocks(&doc, 0).unwrap();
                    let (index, block) = blocks.iter().enumerate().find(|(_, b)| b.text == name).unwrap();
                    let style = text::BlockStyle { align: Some(align), ..Default::default() };
                    text::rewrite_block(&mut doc, 0, index, Some(&block.editor_text), &style).unwrap();
                    doc = reopen(&doc);
                    let title = rect(&doc, name);
                    let error = match align {
                        added::Align::Center => ((title[0] + title[2]) - (column[0] + column[2])).abs() / 2.0,
                        added::Align::Right => (title[2] - column[2]).abs(),
                        _ => (title[0] - column[0]).abs(),
                    };
                    assert!(error < 1.0, "{align:?}, mixed={mixed}, title_y={title_y}: {title:?} must use {column:?}");
                    assert_eq!(rect(&doc, "A page-wide header"), header);
                    assert_eq!(rect(&doc, "A page-wide footer"), footer);
                    assert_eq!(rect(&doc, column_prefix), column);
                }
            }
        }
    }
}

/// How each glyph-showing operator on page 0 is painted, read the way a renderer reads the page:
/// its text, `Tr`, stroke colour operators, line width (from `w` or an ExtGState's `/LW`) and
/// the ExtGStates selected before it, following `q`/`Q`.
#[derive(Clone, Debug, PartialEq)]
struct Painted {
    text: String,
    render_mode: i64,
    stroke: Vec<String>,
    line_width: f64,
    gs: Vec<String>,
}

fn painted_shows(doc: &Document) -> Vec<Painted> {
    let (_, ops) = joined_ops(doc);
    let p = &pdfcraft_model::pages(doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let ext = res.as_dict().unwrap().get(b"ExtGState").map(|g| doc.resolve(g).as_dict().cloned().unwrap()).unwrap_or_default();
    let mut state = Painted { text: String::new(), render_mode: 0, stroke: Vec::new(), line_width: 1.0, gs: Vec::new() };
    let mut stack = Vec::new();
    let mut out = Vec::new();
    let show = |op: &pdfcraft_content::Op| String::from_utf8_lossy(&pdfcraft_content::serialize_ops(std::slice::from_ref(op))).trim().to_string();
    for op in ops {
        match op.op.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => state = stack.pop().unwrap(),
            b"Tr" => state.render_mode = op.operands[0].as_int().unwrap(),
            b"G" | b"RG" | b"K" | b"CS" => state.stroke = vec![show(&op)],
            b"SC" | b"SCN" => state.stroke.push(show(&op)),
            b"w" => state.line_width = op.num(0).unwrap(),
            b"gs" => {
                let name = op.name(0).unwrap();
                state.gs.push(String::from_utf8_lossy(name).into_owned());
                let lw = ext.get(name).and_then(|g| doc.resolve(g).as_dict().and_then(|g| g.get(b"LW")).and_then(|w| w.as_f64()));
                state.line_width = lw.unwrap_or(state.line_width);
            }
            b"Tj" => {
                let Some(Object::String(s)) = op.operands.last() else { continue };
                if !s.bytes.is_empty() {
                    out.push(Painted { text: String::from_utf8_lossy(&s.bytes).into_owned(), ..state.clone() });
                }
            }
            _ => {}
        }
    }
    out
}

/// A page with Helvetica as /F1, /GS1, an ExtGState that paints at 20 % opacity, /GS2, one that
/// sets the line width to 4, /GS3, another at 20 % opacity, and /GS4, at 20 % opacity and dashed.
fn faint_page(content: &str) -> Document {
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R /GS2 7 0 R /GS3 8 0 R /GS4 9 0 R >> >> >>",
        &format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /ExtGState /ca 0.2 >>",
        "<< /Type /ExtGState /LW 4 >>",
        "<< /Type /ExtGState /ca 0.2 >>",
        "<< /Type /ExtGState /ca 0.2 /D [[6 3] 0] >>",
    ])
}

/// Rewriting a paragraph drew it before its text object, restating only the font, spacing and
/// fill: a `1 Tr 1 0 0 RG 2 w` set inside `BT` was lost, and red outline text came back filled
/// black, even with the text unchanged (#3508). Set before `BT` (the control), it already worked.
#[test]
fn a_rewritten_paragraph_keeps_the_rendering_mode_stroke_and_width_set_inside_its_text_object() {
    let outline = |text: &str| Painted { text: text.into(), render_mode: 1, stroke: vec!["1 0 0 RG".into()], line_width: 2.0, gs: Vec::new() };
    for content in ["BT /F1 72 Tf 1 Tr 1 0 0 RG 2 w 40 400 Td (OUTLINE) Tj ET\n", "1 Tr 1 0 0 RG 2 w BT /F1 72 Tf 40 400 Td (OUTLINE) Tj ET\n"] {
        assert_eq!(painted_shows(&text_page(content)), [outline("OUTLINE")], "the fixture: {content}");
        for new in ["OUTLINE", "OUTLINED"] {
            let mut doc = text_page(content);
            text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
            let doc = reopen(&doc);
            assert_eq!(painted_shows(&doc), [outline(new)], "{content} -> {new}: {}", streams(&doc, 0).join("\n"));
        }
    }
}

/// The same for transparency: `/GS1 gs` (`/ca 0.2`) inside `BT` was not in force for the
/// rewritten paragraph, so 20 % blue text came back opaque (#3509).
#[test]
fn a_rewritten_paragraph_keeps_the_extgstate_selected_inside_its_text_object() {
    for content in ["BT /GS1 gs /F1 72 Tf 0 0 1 rg 40 400 Td (FAINT) Tj ET\n", "/GS1 gs BT /F1 72 Tf 0 0 1 rg 40 400 Td (FAINT) Tj ET\n"] {
        for new in ["FAINT", "FAINTER"] {
            let mut doc = faint_page(content);
            text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
            let doc = reopen(&doc);
            let shows = painted_shows(&doc);
            assert_eq!(shows.len(), 1, "{content} -> {new}: {shows:?}");
            assert_eq!(
                (shows[0].text.as_str(), shows[0].gs.as_slice()),
                (new, ["GS1".to_string()].as_slice()),
                "{content} -> {new}: {}",
                streams(&doc, 0).join("\n")
            );
        }
    }
}

/// A paragraph of differently coloured runs goes through the styled rewrite, which draws each
/// run in its own text object: each keeps the state its source set inside `BT`.
#[test]
fn a_styled_rewrite_keeps_the_paint_state_set_inside_the_text_object() {
    let content = "BT /GS1 gs 1 Tr 1 0 0 RG 2 w /F1 72 Tf 40 400 Td (FAI) Tj 0 0 1 rg (NT) Tj ET\n";
    let mut doc = faint_page(content);
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].rows[0].len(), 2, "two runs on one row");
    text::rewrite_block(&mut doc, 0, 0, Some("FAINTS"), &BlockStyle::default()).unwrap();
    let doc = reopen(&doc);
    let shows = painted_shows(&doc);
    let joined = streams(&doc, 0).join("\n");
    assert_eq!(shows.iter().map(|s| s.text.as_str()).collect::<String>(), "FAINTS", "{joined}");
    for s in &shows {
        assert_eq!((s.render_mode, s.stroke.as_slice(), s.line_width), (1, ["1 0 0 RG".to_string()].as_slice(), 2.0), "{s:?}: {joined}");
        assert_eq!(s.gs, ["GS1"], "{s:?}: {joined}");
    }
}

/// A `gs` has no inverse, so one run's ExtGState stayed in force for the runs the styled rewrite
/// drew after it. Two ExtGStates at the same opacity paint the same (see `Paint`), so their runs
/// share a paragraph, but either may set what that doesn't compare (a dash, a line cap): each run
/// is drawn with only the ones its source selected.
#[test]
fn a_styled_rewrite_does_not_carry_one_runs_extgstate_into_the_next() {
    // "FAI" ends at 40 + 72 * (0.611 + 0.667 + 0.278) = 152.032; "NT" is blue, so another run.
    let content = "q BT /GS3 gs /F1 72 Tf 40 400 Td (FAI) Tj ET Q BT /GS1 gs /F1 72 Tf 0 0 1 rg 152.032 400 Td (NT) Tj ET
";
    for new in ["FAINT", "FAINTS"] {
        let mut doc = faint_page(content);
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].rows[0].len(), 2, "two runs on one row");
        text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
        let doc = reopen(&doc);
        let shows = painted_shows(&doc);
        let joined = streams(&doc, 0).join(
            "
",
        );
        assert_eq!(shows.iter().map(|s| s.text.as_str()).collect::<String>(), new, "{joined}");
        let gs: Vec<_> = shows.iter().map(|s| (s.text.as_str(), s.gs.as_slice())).collect();
        assert_eq!(gs.first(), Some(&("FAI", ["GS3".to_string()].as_slice())), "{gs:?}: {joined}");
        assert!(gs.iter().skip(1).all(|(_, gs)| *gs == ["GS1"]), "{gs:?}: {joined}");
    }
}

/// An ExtGState's `/LW` sets the line width as `w` does: `2 w /GS2 gs` (`/LW 4`) outlines at 4,
/// and the rewrite replayed `/GS2 gs` and then `2 w`. `/GS2 gs 1 w` outlines at 1, and the
/// rewrite replayed only `/GS2 gs`, since 1 is the initial width.
#[test]
fn a_rewritten_paragraph_keeps_the_line_width_an_extgstate_set() {
    for (content, width) in [
        ("BT /F1 72 Tf 1 Tr 2 w /GS2 gs 40 400 Td (OUTLINE) Tj ET\n", 4.0),
        ("BT /F1 72 Tf 1 Tr /GS2 gs 2 w 40 400 Td (OUTLINE) Tj ET\n", 2.0),
        ("BT /F1 72 Tf 1 Tr /GS2 gs 1 w 40 400 Td (OUTLINE) Tj ET\n", 1.0),
    ] {
        assert_eq!(painted_shows(&faint_page(content))[0].line_width, width, "the fixture: {content}");
        for new in ["OUTLINE", "OUTLINED"] {
            let mut doc = faint_page(content);
            text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
            let doc = reopen(&doc);
            let shows = painted_shows(&doc);
            assert_eq!(
                shows.iter().map(|s| (s.text.as_str(), s.render_mode, s.line_width)).collect::<Vec<_>>(),
                [(new, 1, width)],
                "{content} -> {new}: {}",
                streams(&doc, 0).join("\n")
            );
        }
    }
}

/// A line is rewritten in the state of its first run, so outline text partway through a text
/// object came back filled: "FAI" filled and "NT" a red outline came back all filled. The change
/// of `Tr` now starts a run. (A `gs` partway already starts a paragraph; see `Paint`.)
#[test]
fn a_rendering_mode_partway_through_a_line_starts_a_run_of_its_own() {
    let content = "BT /F1 72 Tf 40 400 Td (FAI) Tj 1 Tr 1 0 0 RG 2 w (NT) Tj ET
";
    for new in ["FAINT", "FAINTS"] {
        let mut doc = faint_page(content);
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].rows[0].len(), 2, "two runs on one row");
        text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
        let doc = reopen(&doc);
        let shows = painted_shows(&doc);
        let joined = streams(&doc, 0).join(
            "
",
        );
        assert_eq!(shows.iter().map(|s| s.text.as_str()).collect::<String>(), new, "{joined}");
        let modes: Vec<_> = shows.iter().map(|s| (s.text.as_str(), s.render_mode, s.stroke.as_slice(), s.line_width)).collect();
        assert_eq!(modes.first().map(|m| (m.0, m.1)), Some(("FAI", 0)), "{modes:?}: {joined}");
        let outline = ["1 0 0 RG".to_string()];
        assert!(modes.iter().skip(1).all(|m| (m.1, m.2, m.3) == (1, outline.as_slice(), 2.0)), "{modes:?}: {joined}");
    }
}

/// Two ExtGStates at the same opacity paint text the same (see `Paint`), but one of them dashes
/// its outline: a dashed outline after a solid one in the same text object joined its line and
/// came back solid, drawn with the first run's selections. A different dash now starts a run.
#[test]
fn a_dash_partway_through_an_outlined_line_starts_a_run_of_its_own() {
    let content = "BT /GS3 gs /F1 72 Tf 1 Tr 1 0 0 RG 2 w 40 400 Td (FAI) Tj /GS4 gs (NT) Tj ET
";
    for new in ["FAINT", "FAINTS"] {
        let mut doc = faint_page(content);
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].rows[0].len(), 2, "two runs on one row");
        text::rewrite_block(&mut doc, 0, 0, Some(new), &BlockStyle::default()).unwrap();
        let doc = reopen(&doc);
        let shows = painted_shows(&doc);
        let joined = streams(&doc, 0).join(
            "
",
        );
        assert_eq!(shows.iter().map(|s| s.text.as_str()).collect::<String>(), new, "{joined}");
        let gs: Vec<_> = shows.iter().map(|s| (s.text.as_str(), s.gs.as_slice())).collect();
        assert_eq!(gs.first(), Some(&("FAI", ["GS3".to_string()].as_slice())), "{gs:?}: {joined}");
        assert!(gs.iter().skip(1).all(|(_, gs)| gs.last().is_some_and(|g| g == "GS4")), "{gs:?}: {joined}");
    }
}

/// The dash and cap are compared as the operators set them too: `d` or `J` partway through an
/// outlined line starts a run, and a malformed one (too long, not numbers, out of range) is ignored.
#[test]
fn a_dash_or_cap_operator_partway_through_an_outlined_line_starts_a_run() {
    let long = format!("[{}] 0 d", "1 ".repeat(40));
    for (change, runs) in [("[6 3] 0 d", 2), ("1 J", 2), (long.as_str(), 1), ("[(a) 3] 0 d", 1), ("[6 3] /x d", 1), ("7 J", 1), ("J", 1)] {
        let content = format!("BT /F1 72 Tf 1 Tr 1 0 0 RG 2 w 40 400 Td (FAI) Tj {change} (NT) Tj ET\n");
        let mut doc = faint_page(&content);
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].rows[0].len(), runs, "{change}");
        text::rewrite_block(&mut doc, 0, 0, Some("FAINTS"), &BlockStyle::default()).unwrap();
        let shows = painted_shows(&reopen(&doc));
        assert_eq!(shows.iter().map(|s| s.text.as_str()).collect::<String>(), "FAINTS", "{change}");
    }
}

/// #4199, #3797: every space typed into a paragraph is saved, not folded into one.
#[test]
fn an_edited_paragraph_keeps_every_space_typed() {
    for typed in ["Hello  world", "Hello     world   end", "  indented and trailing  "] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
        assert_eq!(text::replace_block(&mut doc, 0, 0, typed).unwrap().substituted, None);
        let doc = reopen(&doc);
        let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
        assert!(content.contains(&format!("({typed}) Tj")), "{typed:?}: {content}");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, typed);
    }
}

/// Spaces after a row's last word are kept but take no part in its alignment: a justified row
/// still reaches the margin, and a centred or right-aligned one sits where it would without them.
#[test]
fn spaces_after_a_rows_last_word_do_not_move_its_alignment() {
    let shown = |text: &str, align: added::Align| {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
        let style =
            text::BlockStyle { family: Some((added::Family::Helvetica, false, false)), align: Some(align), width: Some(300.0), ..Default::default() };
        text::rewrite_block(&mut doc, 0, 0, Some(text), &style).unwrap();
        String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned()
    };
    for (plain, trailing, align) in [
        ("aaa bbb ccc\nddd eee", "aaa bbb ccc  \nddd eee", added::Align::Justify),
        ("Title", "Title   ", added::Align::Center),
        ("Title", "Title   ", added::Align::Right),
    ] {
        let first = trailing.split('\n').next().unwrap();
        let expected = shown(plain, align);
        let actual = shown(trailing, align).replace(&format!("({first})"), &format!("({})", first.trim_end()));
        assert_eq!(actual, expected, "{trailing:?} {align:?}");
    }
}

/// A justified row runs from the paragraph's left edge to the margin. Word spacing reaches only
/// the spaces between its words: the spaces before its first word and after its last are left
/// out of a stretched row (as before #4199), so the reopened rows stay inside the paragraph and
/// can be edited again in place.
#[test]
fn a_justified_row_keeps_its_indent_and_ends_at_the_margin() {
    let style = text::BlockStyle {
        family: Some((added::Family::Helvetica, false, false)),
        align: Some(added::Align::Justify),
        width: Some(300.0),
        ..Default::default()
    };
    for typed in ["    Pack my box with five dozen jugs\nof liquor", "    Pack my box with five dozen jugs   \nof liquor"] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
        text::rewrite_block(&mut doc, 0, 0, Some(typed), &style).unwrap();
        let mut doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        let first = lines.iter().find(|l| l.text.contains("Pack")).unwrap();
        assert_eq!(first.text, "Pack my box with five dozen jugs", "{typed:?}");
        assert!((first.rect[0] - 72.0).abs() < 0.5, "{typed:?}: {:?}", first.rect);
        assert!(lines.iter().all(|l| l.rect[0] >= 72.0 - 0.5 && l.rect[2] <= 372.5), "{typed:?}: {lines:?}");
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1, "{typed:?}: {blocks:?}");
        text::replace_block(&mut doc, 0, 0, "Edited again").unwrap();
        let again = text::text_lines(&reopen(&doc), 0).unwrap();
        assert!(again.iter().all(|l| l.rect[0] >= 0.0 && l.rect[2] <= 600.0), "{typed:?}: {again:?}");
    }
}

/// A justified paragraph whose first row was typed with an indent is edited again through the
/// styled path (a draft with a line break): its rows keep their scaling and stay one paragraph
/// inside its width.
#[test]
fn a_justified_paragraph_with_an_indent_edits_again_at_its_own_scale() {
    let style = text::BlockStyle {
        family: Some((added::Family::Helvetica, false, false)),
        align: Some(added::Align::Justify),
        width: Some(300.0),
        ..Default::default()
    };
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("    aaa bbb ccc  \nddd"), &style).unwrap();
    let mut doc = reopen(&doc);
    let draft = text::text_blocks(&doc, 0).unwrap()[0].editor_text.replacen("ccc", "cc", 1);
    assert!(draft.contains('\n'), "{draft:?}");
    text::replace_block(&mut doc, 0, 0, &draft).unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    let scales: Vec<&str> = content.lines().filter(|l| l.ends_with(" Tz")).collect();
    assert!(scales.iter().all(|l| *l == "100 Tz"), "{scales:?}: {content}");
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.iter().all(|l| l.rect[0] >= 72.0 - 0.5 && l.rect[2] <= 372.5), "{lines:?}");
}

/// A paragraph drawn at `0 Tz` has no width to justify into: nothing is divided by it. The
/// writer writes a number that isn't finite as 0, so the test looks for the word spacing and
/// the move a division by zero would add, rather than for `NaN`.
#[test]
fn justifying_text_at_zero_horizontal_scaling_divides_by_nothing() {
    let mut doc = text_page("BT /F1 12 Tf 0 Tz 72 700 Td (Hello world) Tj ET");
    let style = text::BlockStyle { family: Some((added::Family::Helvetica, false, false)), align: Some(added::Align::Justify), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("    aaa bbb\nccc"), &style).unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    let block = content.split("ET").next().unwrap();
    assert_eq!(block.matches(" Tw").count(), 1, "only the paragraph's own 0 Tw: {content}");
    let first_row = block.split(" Tm").nth(1).and_then(|rest| rest.split(" Tj").next()).unwrap();
    assert!(!first_row.contains(" Td"), "no move before the first row: {content}");
}

/// The preview's box is the box of the rows Apply writes, measured to their last glyphs.
#[test]
fn a_drafts_box_is_the_box_apply_draws() {
    let doc = text_page("BT /F1 12 Tf 14 TL 72 700 Td [(First) -2000 (row here )] TJ T* (Second row ) Tj ET");
    let source = text::text_blocks(&doc, 0).unwrap().swap_remove(0);
    let draft = source.editor_text.replacen("Second", "Third", 1);
    let layout = text::text_edit_layout(&doc, 0, 0, &draft, &text::BlockStyle::default()).unwrap();
    let mut applied = doc.clone();
    text::replace_block(&mut applied, 0, 0, &draft).unwrap();
    let boxed = text::text_lines(&applied, 0)
        .unwrap()
        .into_iter()
        .map(|l| {
            let end = l.ink.iter().map(|r| r[2]).fold(f64::NEG_INFINITY, f64::max);
            [l.rect[0], l.rect[1], if end.is_finite() { end.min(l.rect[2]) } else { l.rect[2] }, l.rect[3]]
        })
        .reduce(|a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
    let (d, b) = (layout.draft_rect.unwrap(), boxed.unwrap());
    assert!(d.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01), "{draft:?}: preview {d:?}, applied {b:?}");
}

/// The draft's box is measured to its last glyph, not over the spaces typed after it.
#[test]
fn a_drafts_box_leaves_out_spaces_after_the_last_word() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
    let layout = |text: &str| text::text_edit_layout(&doc, 0, 0, text, &text::BlockStyle::default()).unwrap().draft_rect;
    assert_eq!(layout("Hello there     "), layout("Hello there"));
}

/// A lone carriage return (an old Mac line ending) breaks the row, as `\n` and `\r\n` do.
#[test]
fn a_lone_carriage_return_breaks_an_edited_paragraph() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
    text::replace_block(&mut doc, 0, 0, "one\rtwo").unwrap();
    let lines: Vec<String> = text::text_lines(&reopen(&doc), 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(lines, ["one", "two"]);
}

/// On the styled rewrite path too, `\r\n` is one row break and a lone `\r` another, and no
/// carriage return reaches the page: with no other formatting, and with a part set in red.
#[test]
fn carriage_returns_break_rows_on_the_styled_path() {
    let draft = "one\r\ntwo\rthree";
    let red = text::RangeStyle {
        range: 0..3,
        family: None,
        resource: None,
        size: None,
        color: Some([1.0, 0.0, 0.0]),
        underline: None,
        char_spacing: None,
        scale: None,
    };
    for style in [text::BlockStyle::default(), text::BlockStyle { ranges: vec![red], ..Default::default() }] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello world) Tj ET");
        text::rewrite_block(&mut doc, 0, 0, Some(draft), &style).unwrap();
        let doc = reopen(&doc);
        let content = page_content_bytes(&doc, 0);
        assert!(!content.contains(&b'\r') && !content.windows(2).any(|w| w == b"\\r"), "{}", String::from_utf8_lossy(&content));
        let lines: Vec<String> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
        assert_eq!(lines, ["one", "two", "three"], "{style:?}");
    }
}

/// A subset with no space glyph shows each space as a positioning gap one space wide, so a run
/// of spaces still takes a run's width (#4199).
#[test]
fn a_run_of_spaces_in_a_subset_without_a_space_glyph_keeps_its_width() {
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj /F1 12 Tf ( ) Tj /F2 12 Tf (ba) Tj ET");
    assert_eq!(text::replace_block(&mut doc, 0, 0, "ab  ba   ab").unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&reopen(&doc), 0)).into_owned();
    assert_eq!(content.matches("[-330] TJ").count(), 5, "{content}");
}

/// Inside a text object `q`/`Q` save and restore the text and text line matrices (ISO 32000-2
/// §9.4.1, as corrected by the PDF Association's errata, issue 368), as the renderer does: the
/// text after `Q` is found, and stays, where it is drawn.
#[test]
fn q_inside_a_text_object_restores_the_text_matrices() {
    for (content, text, baseline) in [
        ("BT /F1 12 Tf 72 700 Td q 0 -100 Td Q (After) Tj ET", "After", [72.0, 700.0]),
        ("BT /F1 12 Tf 72 700 Td q 1 0 0 1 300 300 Tm Q (After) Tj ET", "After", [72.0, 700.0]),
        // `T*` moves from the restored line, not the one set inside q/Q.
        ("BT /F1 12 Tf 14 TL 72 700 Td q 0 -100 Td Q T* (Next) Tj ET", "Next", [72.0, 686.0]),
    ] {
        let mut doc = text_page(content);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!((lines[0].text.as_str(), lines[0].baseline()), (text, baseline), "{content}");
        text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
        let doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!((lines[0].text.as_str(), lines[0].baseline()), ("Edited", baseline), "{content} after editing");
    }
}

/// Whole-paragraph alignment retains both soft wraps and typed hard breaks after save/reopen.
/// Selected-row alignment for existing PDF content is withheld until it can persist safely.
#[test]
fn wrapped_rows_keep_whole_paragraph_alignment_and_hard_break_markers() {
    let mut doc = text_page("BT /F1 10 Tf 12 TL 72 700 Td (Old) Tj ET");
    let style = text::BlockStyle { width: Some(60.0), align: Some(added::Align::Left), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("One two three four five six\nTail"), &style).unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.len() > 2, "the first logical row must wrap");
    let tail = lines.iter().find(|line| line.text == "Tail").unwrap();
    assert!((tail.rect[0] - 72.0).abs() < 1.0, "the second logical row keeps the paragraph alignment: {:?}", tail.rect);
    for line in lines.iter().filter(|line| line.text != "Tail") {
        assert!((line.rect[0] - 72.0).abs() < 1.0, "soft-wrapped rows stay left-aligned: {:?}", line.rect);
    }
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("/PCSoft") && content.contains("/PCHard"), "both break kinds survive: {content}");
}

#[test]
fn hostile_soft_wrap_counts_fail_before_reflow_allocates() {
    for marker in [
        "/PCSoft << /N 9223372036854775807 >> BDC EMC",
        "/PCSoft << /N 16777217 >> BDC EMC",
        "/PCSoft << /N 8388608 >> BDC EMC /PCSoft << /N 8388609 >> BDC EMC",
    ] {
        let doc = text_page(&format!("BT /F1 12 Tf 20 700 Td (First) Tj {marker} 0 -14 Td (Second) Tj ET"));
        let error = text::text_lines(&doc, 0).unwrap_err();
        assert!(error.to_string().contains("too large"), "{marker}: {error}");
    }
}

#[test]
fn ordinary_soft_wrap_counts_keep_their_spaces() {
    for gap in [0, 1, 2, 8] {
        let doc = text_page(&format!("BT /F1 12 Tf 20 700 Td (First) Tj /PCSoft << /N {gap} >> BDC EMC 0 -14 Td (Second) Tj ET"));
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 2);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].reflow_text, format!("First{}Second", " ".repeat(gap)));
    }
}

#[test]
fn styled_existing_justification_is_refused_instead_of_silently_left_aligning() {
    let mut doc = mixed_region_page();
    let before = page_content_bytes(&doc, 0);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let style = text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() };
    let error = text::rewrite_block(&mut doc, 0, 1, Some(&blocks[1].editor_text), &style).unwrap_err();
    assert!(error.to_string().contains("justification"));
    assert_eq!(page_content_bytes(&doc, 0), before);
    assert_eq!(page_content_bytes(&reopen(&doc), 0), before);
}
