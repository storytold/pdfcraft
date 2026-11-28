//! Opacity of one page image or placed form: what renders, what `image_opacities` reads back, and
//! how it lives with the other image edits.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, SaveOptions, write_incremental};
use pdfcraft_edit::{ArrangeOrder, ImageChange, change_image, image_opacities, page_images, rect_to_rect};

/// A 200 × 100 page: a black picture /Im0 on the left half and a black form /Fm0 on the right
/// half, drawn by `content`; `extra` adds entries to the page's `/ExtGState`.
fn page(content: &str, extra: &str) -> Document {
    let form = "0 g 0 0 100 100 re f";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R /Fm0 6 0 R >> /ExtGState << /Half << /Type /ExtGState /ca 0.5 /CA 0.5 >> {extra} >> >> >>"
        ),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
            .into(),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{form}\nendstream", form.len()),
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

const BOTH: &str = "q 100 0 0 100 0 0 cm /Im0 Do Q q 1 0 0 1 100 0 cm /Fm0 Do Q";

/// The grey level (red channel) at user-space point (x, y) of the page rendered at 72 dpi.
fn grey(doc: &Document, x: u32, y: u32) -> u8 {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let page =
        renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, scale: 1.0, ..Default::default() });
    page.rgba[(((100 - y) * page.width + x) * 4) as usize]
}

/// How many times each of `ops` appears as an operator in page 1's content.
fn count(doc: &Document, ops: &[&str]) -> Vec<usize> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let refs = match &*doc.resolve(p.dict.get(b"Contents").unwrap()) {
        Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
        _ => p.dict.get(b"Contents").and_then(Object::as_ref).into_iter().collect::<Vec<_>>(),
    };
    let mut text = String::new();
    for r in refs {
        if let Object::Stream(s) = &*doc.get(r) {
            text.push_str(&String::from_utf8_lossy(&s.decoded().unwrap()));
            text.push(' ');
        }
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    ops.iter().map(|op| words.iter().filter(|w| *w == op).count()).collect()
}

fn opacity(doc: &mut Document, index: usize, alpha: f64) {
    change_image(doc, 0, index, &ImageChange::Opacity(alpha)).unwrap();
}

#[test]
fn a_picture_set_to_half_opacity_lets_the_paper_show_through_and_reads_back() {
    let mut doc = page(BOTH, "");
    assert!(grey(&doc, 50, 50) < 20, "the picture starts black");
    opacity(&mut doc, 0, 0.5);
    let half = grey(&doc, 50, 50);
    assert!((100..160).contains(&half), "half-transparent black over white is mid grey, got {half}");
    assert!(grey(&doc, 150, 50) < 20, "the form beside it is not changed");
    let read = image_opacities(&doc, 0).unwrap();
    assert!((read[0] - 0.5).abs() < 1e-6 && (read[1] - 1.0).abs() < 1e-6, "{read:?}");
}

#[test]
fn setting_opacity_again_replaces_it_instead_of_stacking() {
    let mut doc = page(BOTH, "");
    let plain = count(&doc, &["q", "Q"]);
    opacity(&mut doc, 0, 0.5);
    assert!(change_image(&mut doc, 0, 0, &ImageChange::Opacity(0.5)).is_err(), "already at 50%: nothing to change");
    opacity(&mut doc, 0, 0.25);
    opacity(&mut doc, 0, 0.75);
    let three = grey(&doc, 50, 50);
    assert!((30..100).contains(&three), "the last value set is the one drawn, got {three}");
    assert_eq!(count(&doc, &["gs"]), [1], "one graphics state, not one per change");
    opacity(&mut doc, 0, 1.0);
    assert!(grey(&doc, 50, 50) < 20, "100% makes it opaque again");
    assert_eq!(count(&doc, &["gs"]), [0], "and takes our state away");
    opacity(&mut doc, 0, 0.5);
    opacity(&mut doc, 0, 1.0);
    assert_eq!(count(&doc, &["q", "Q", "gs"]), [plain[0], plain[1], 0], "no wrappers pile up: the page is as it was");
    assert!((image_opacities(&doc, 0).unwrap()[0] - 1.0).abs() < 1e-6);
    assert!(change_image(&mut doc, 0, 0, &ImageChange::Opacity(1.0)).is_err(), "already opaque: nothing to change");
}

#[test]
fn placed_artwork_takes_the_opacity() {
    let mut doc = page(BOTH, "");
    opacity(&mut doc, 1, 0.25);
    let light = grey(&doc, 150, 50);
    assert!(light > 160, "a quarter-opaque black form is light grey, got {light}");
    assert!(grey(&doc, 50, 50) < 20, "the picture beside it is not changed");
}

#[test]
fn full_opacity_overrides_an_inherited_translucent_state() {
    let mut doc = page("/Half gs q 100 0 0 100 0 0 cm /Im0 Do Q q 1 0 0 1 100 0 cm /Fm0 Do Q", "");
    assert!((image_opacities(&doc, 0).unwrap()[0] - 0.5).abs() < 1e-6, "the page draws it at 50%");
    opacity(&mut doc, 0, 1.0);
    assert!(grey(&doc, 50, 50) < 20, "set to 100%, it is opaque though the page state is 50%");
    assert!(grey(&doc, 150, 50) > 100, "the form after it keeps the page's 50%");
}

#[test]
fn the_opacity_survives_moving_and_arranging_the_picture() {
    let mut doc = page(BOTH, "");
    opacity(&mut doc, 0, 0.5);
    let r = page_images(&doc, 0).unwrap()[0].rect;
    change_image(&mut doc, 0, 0, &ImageChange::Transform(rect_to_rect(r, [10.0, 10.0, 60.0, 60.0]))).unwrap();
    let moved = grey(&doc, 30, 30);
    assert!((100..160).contains(&moved), "still half-transparent after the move, got {moved}");
    change_image(&mut doc, 0, 0, &ImageChange::Arrange(ArrangeOrder::BringToFront)).unwrap();
    let front = page_images(&doc, 0).unwrap();
    let picture = front.iter().position(|i| !i.is_form).unwrap();
    assert!((image_opacities(&doc, 0).unwrap()[picture] - 0.5).abs() < 1e-6, "arranged, it keeps its opacity");
}

#[test]
fn a_name_taken_by_another_state_is_not_overwritten() {
    let mut doc = page(BOTH, "/PCAlpha500 << /Type /ExtGState /ca 0.9 /BM /Multiply >>");
    opacity(&mut doc, 0, 0.5);
    let half = grey(&doc, 50, 50);
    assert!((100..160).contains(&half), "the new state is used, got {half}");
    let p = pdfcraft_model::pages(&doc).into_iter().next().unwrap();
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let states = doc.resolve(res.get(b"ExtGState").unwrap()).as_dict().cloned().unwrap();
    let kept = doc.resolve(states.get(b"PCAlpha500").unwrap()).as_dict().cloned().unwrap();
    assert!(kept.get(b"BM").is_some(), "the state already there keeps its blend mode");
}

#[test]
fn the_last_thing_on_the_page_gets_a_closed_scope() {
    // The form is the page's last operator, with no `Q` after it.
    let mut doc = page("q 100 0 0 100 0 0 cm /Im0 Do Q 1 0 0 1 100 0 cm /Fm0 Do", "");
    opacity(&mut doc, 1, 0.5);
    let half = grey(&doc, 150, 50);
    assert!((100..160).contains(&half), "the form is half transparent, got {half}");
    let [q, big_q] = count(&doc, &["q", "Q"])[..] else { unreachable!() };
    assert_eq!(q, big_q, "every q is closed");
    opacity(&mut doc, 1, 0.25);
    assert_eq!(count(&doc, &["gs"]), [1], "its own scope is found and replaced");
    assert!(grey(&doc, 50, 50) < 20, "the picture before it is not changed");
}

#[test]
fn opacity_outside_zero_to_one_is_refused() {
    let mut doc = page(BOTH, "");
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.5] {
        assert!(change_image(&mut doc, 0, 0, &ImageChange::Opacity(bad)).is_err(), "{bad} must be refused");
    }
    assert!(grey(&doc, 50, 50) < 20, "nothing changed");
}
