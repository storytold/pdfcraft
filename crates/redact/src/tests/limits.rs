//! Work and memory stay bounded by what the page declares, whatever the file holds.

use super::*;

/// A half-covered 4×1 grey image whose Flate data decodes to `decoded`.
fn half_covered_image(decoded: &[u8]) -> Report {
    let dict = "/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut doc = one_page(b"q 40 0 0 10 100 100 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(dict, b"")]);
    let Object::Stream(s) = &*doc.get(ObjRef::new(7, 0)) else { panic!() };
    doc.set(ObjRef::new(7, 0), Object::Stream(Stream::flate(s.dict.clone(), decoded)));
    mark(&mut doc, 0, &[[95.0, 95.0, 120.0, 115.0]], "");
    apply(&mut doc, None).unwrap()
}

#[test]
fn an_image_decoding_far_past_its_raster_is_removed_without_decoding_it_all() {
    let r = half_covered_image(&[255; 4]);
    assert_eq!((r.images_removed, r.images_cleared), (0, 1), "{r:?}");
    // 4 bytes of raster, then 4 MiB the image never uses: decoding stops at a bound derived from
    // the raster, and the image goes whole.
    let mut bomb = vec![255; 4];
    bomb.resize(4 << 20, 0);
    let r = half_covered_image(&bomb);
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
}

/// Marked content nested `depth` deep, each level with its own MCID, around `depth` drawing
/// operators.
fn nested_marked_content(depth: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for id in 0..depth {
        data.extend_from_slice(format!("/P <</MCID {id}>> BDC\n").as_bytes());
    }
    data.extend(b"0 0 1 1 re f\n".repeat(depth));
    data.extend(b"EMC\n".repeat(depth));
    data
}

#[test]
fn comparing_deeply_nested_marked_content_holds_each_operator_once() {
    let doc = one_page(b"", "", vec![]);
    let data = nested_marked_content(2000);
    let held = tags::retained_for_comparison(&doc, &Dict::new(), &data);
    assert!(held <= 2 * data.len(), "{held} bytes held for {} bytes of content", data.len());
}

#[test]
fn a_change_inside_nested_marked_content_touches_every_enclosing_mcid() {
    let doc = one_page(b"", "", vec![]);
    let before = b"/P <</MCID 0>> BDC /P <</MCID 1>> BDC BT (A) Tj ET EMC 0 0 1 1 re f EMC /P <</MCID 2>> BDC BT (B) Tj ET EMC";
    let after = b"/P <</MCID 0>> BDC /P <</MCID 1>> BDC BT ET EMC 0 0 1 1 re f EMC /P <</MCID 2>> BDC BT (B) Tj ET EMC";
    let (changed, empty) = tags::touched(&doc, &Dict::new(), before, after);
    assert_eq!(changed, HashSet::from([0, 1]));
    assert_eq!(empty, HashSet::from([1]));
}
