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

#[test]
fn verification_counts_an_unreadable_soft_mask_group_under_a_mark() {
    // A group whose stream does not decode: apply refuses the page, so verification must not
    // report it clean.
    let group = stream("/Type /XObject /Subtype /Form /BBox [0 0 150 150] /Filter /FlateDecode", b"not deflate data SMASKSECRET");
    let mut doc =
        one_page(b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q", "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >>", vec![group]);
    assert!(under(&mut doc, 0, &[MARK]) > 0, "an unreadable group under a mark counts as what may remain");
    assert_eq!(under(&mut doc, 0, &[[200.0, 200.0, 250.0, 250.0]]), 0, "a group no mark meets is not counted");
    mark(&mut doc, 0, &[MARK], "");
    assert!(apply(&mut doc, None).is_err(), "apply refuses the page");
}

#[test]
fn a_soft_mask_group_without_resources_does_not_keep_the_original_through_the_copied_ones() {
    // The group draws with the page's resources, which list the state naming the original group,
    // and an image the group does not draw, so the rewritten group gets a pruned copy of them.
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >>",
        b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
    );
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"\x00");
    let mut doc = one_page(
        b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q q 10 0 0 10 250 250 cm /Im0 Do Q",
        "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >> /XObject << /Im0 8 0 R >>",
        vec![group, image],
    );
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 11, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the original group survived through the rewritten group's resources");
}

/// `masked_page` and a second page, both inheriting the font and the soft-mask state from the
/// page tree; the second page paints through the mask too when `again`.
fn two_pages_inheriting_the_mask(again: bool) -> Document {
    let second: &[u8] = if again { b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q" } else { b"0 0 0.6 rg 0 0 10 10 re f" };
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /Resources << /Font << /F1 5 0 R >> /ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >>",
            b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
        ),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 9 0 R >>".to_vec(),
        stream("", second),
    ])
}

#[test]
fn an_inherited_soft_mask_state_does_not_keep_the_original_group() {
    let mut doc = two_pages_inheriting_the_mask(false);
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 11, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the original group survived through the shared resources");

    // A page that still paints through the original keeps it: nothing of it lies under a mark there.
    let mut doc = two_pages_inheriting_the_mask(true);
    mark(&mut doc, 0, &[MARK], "");
    apply(&mut doc, None).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the second page's mask lost its group");
}

/// A form the page draws shifted clear of the mark, whose own resources list a state naming the
/// original group: `again` makes the form select it.
fn a_form_naming_the_mask(again: bool) -> Document {
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >>",
        b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
    );
    let body: &[u8] = if again { b"q /GSF gs 0 0 0.6 rg 200 200 20 20 re f Q" } else { b"0 0 0.6 rg 200 200 20 20 re f" };
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Matrix [1 0 0 1 150 0] /Resources << /ExtGState << /GSF << /SMask << /S /Luminosity /G 7 0 R >> >> >> >>",
        body,
    );
    one_page(
        b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q /Fm1 Do",
        "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >> /XObject << /Fm1 8 0 R >>",
        vec![group, form],
    )
}

#[test]
fn a_form_resource_state_does_not_keep_the_original_group() {
    let mut doc = a_form_naming_the_mask(false);
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 11, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the original group survived through a form's own resources");

    // A form that still paints through the original keeps it.
    let mut doc = a_form_naming_the_mask(true);
    mark(&mut doc, 0, &[MARK], "");
    apply(&mut doc, None).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the form's mask lost its group");
}

#[test]
fn a_state_inside_a_surviving_soft_mask_group_does_not_keep_the_original_group() {
    // GS1's group (object 8) holds no text, so it survives as it is; its own resources list an
    // unused state naming the original group (object 7).
    let other = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /ExtGState << /GSI << /SMask << /S /Luminosity /G 7 0 R >> >> >> >>",
        b"1 g 0 0 300 300 re f",
    );
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >>",
        b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
    );
    let mut doc = one_page(
        b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q q /GS1 gs 0 0 0.6 rg 200 200 20 20 re f Q",
        "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> /GS1 << /SMask << /S /Luminosity /G 8 0 R >> >> >>",
        vec![group, other],
    );
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 11, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"SMASKSECRET"), "the original group survived through another soft-mask group's resources");
}

#[test]
fn a_kept_original_group_does_not_keep_another_original_through_its_resources() {
    // Page 1 paints through two masks whose groups (7 and 8) both hold text under the mark, so
    // both are rewritten. Page 2 still paints through the original group 7, which keeps it; group
    // 7's own resources list an unused state naming the original group 8.
    let first = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> /ExtGState << /GSB << /SMask << /S /Luminosity /G 8 0 R >> >> >> >>",
        b"1 g BT /F1 10 Tf 10 100 Td (SMASKSECRET) Tj ET",
    );
    let second = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >>",
        b"1 g BT /F1 10 Tf 10 100 Td (BMASKSECRET) Tj ET",
    );
    let masks = "/ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> /GS1 << /SMask << /S /Luminosity /G 8 0 R >> >> >>";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {masks} >> >>").into_bytes(),
        stream("", b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q q /GS1 gs 0 0 0.6 rg 0 0 300 300 re f Q"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        first,
        second,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 10 0 R /Resources << /ExtGState << /GS0 << /SMask << /S /Luminosity /G 7 0 R >> >> >> >> >>".to_vec(),
        stream("", b"q /GS0 gs 0 0 0.6 rg 0 0 300 300 re f Q"),
    ]);
    mark(&mut doc, 0, &[MARK], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 22, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes.windows(11).any(|w| w == b"SMASKSECRET"), "page 2's mask lost its group");
    assert!(!bytes.windows(11).any(|w| w == b"BMASKSECRET"), "the second original group survived through the kept one's resources");
}
