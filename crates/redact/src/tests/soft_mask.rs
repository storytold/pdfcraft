//! Text drawn in the group of an ExtGState's soft mask shows through what the mask lets paint, so
//! a mark over it reaches it (#7884).

use super::*;

/// Over "SMASKSECRET" at (10, 100), 10 pt: 11 glyphs, 5 pt each.
const MARK: [f64; 4] = [5.0, 95.0, 70.0, 110.0];

/// A page that paints through a soft mask whose luminosity group (object 7) shows the text.
fn masked_page() -> Document {
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >>",
        b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
    );
    one_page(b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q", "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >>", vec![group])
}

#[test]
fn verification_counts_text_in_a_soft_mask_group_under_a_mark() {
    let mut doc = masked_page();
    assert_eq!(under(&mut doc, 0, &[MARK]), 11);
    assert_eq!(under(&mut doc, 0, &[[200.0, 200.0, 250.0, 250.0]]), 0, "a mark elsewhere meets none of it");
}

#[test]
fn applying_removes_text_in_a_soft_mask_group_and_keeps_the_mask() {
    let mut doc = masked_page();
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (11, 1), "{r:?}");

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the group's text survived the save");

    // The page still paints through a soft mask, now with the cleaned group.
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let res = saved.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let states = saved.resolve(res.get(b"ExtGState").unwrap()).as_dict().cloned().unwrap();
    assert!(!states.contains(b"GS0"), "the original state, which names the original group, is gone: {states:?}");
    let (name, state) = states.iter().next().expect("the copy is listed");
    let c = content(&saved, 0);
    assert!(c.contains(&format!("/{} gs", String::from_utf8_lossy(name))), "{c}");
    let mask = saved.resolve(saved.resolve(state).as_dict().unwrap().get(b"SMask").unwrap()).as_dict().cloned().unwrap();
    let Object::Stream(group) = &*saved.get(mask.get(b"G").and_then(Object::as_ref).unwrap()) else { panic!("the group is a stream") };
    let text = String::from_utf8_lossy(&group.decoded().unwrap()).into_owned();
    assert!(text.contains("BT") && !text.contains("SMASK"), "{text}");
}
