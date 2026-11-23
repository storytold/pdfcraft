//! Preseparated pages (ISO 32000-1 §14.11.4): each page of a set lists the set's pages in its
//! `/SeparationInfo /Pages`, and every page operation must leave that list naming pages of the
//! same document that list it back.

use super::*;

/// The plates of one preseparated page (Cyan, Magenta, Yellow, Black: pages 0–3), then an
/// ordinary page (4). With `shared`, the plates share one indirect separation dictionary;
/// otherwise each has its own, holding the same list.
fn plates(shared: bool) -> Document {
    let info = |colorant: &str| {
        format!("<< /Pages [3 0 R 4 0 R 5 0 R 6 0 R] /DeviceColorant /{colorant} /ColorSpace [/Separation /{colorant} /DeviceCMYK 13 0 R] >>")
    };
    let plate = |content: u32, colorant: &str| {
        let info = if shared { "14 0 R".to_string() } else { info(colorant) };
        format!("<< /Type /Page /Parent 2 0 R /Contents {content} 0 R /SeparationInfo {info} >>")
    };
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),                                                         // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R 7 0 R] /Count 5 /MediaBox [0 0 200 200] >>".into(), // 2
        plate(8, "Cyan"),                                                                                   // 3
        plate(9, "Magenta"),                                                                                // 4
        plate(10, "Yellow"),                                                                                // 5
        plate(11, "Black"),                                                                                 // 6
        "<< /Type /Page /Parent 2 0 R /Contents 12 0 R >>".into(),                                          // 7
        body("C"),                                                                                          // 8
        body("M"),                                                                                          // 9
        body("Y"),                                                                                          // 10
        body("K"),                                                                                          // 11
        body("plain"),                                                                                      // 12
        "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>".into(),                      // 13
        info("All"),                                                                                        // 14
    ];
    open(build(&b, "/Root 1 0 R"))
}

/// The separation dictionary of page `index`, resolved.
fn separation(doc: &Document, index: usize) -> Option<Dict> {
    let page = pages(doc).unwrap()[index].obj;
    let info = doc.get(page).as_dict()?.get(b"SeparationInfo").cloned()?;
    doc.dict(&info)
}

/// What page `index`'s set lists, as page indices of `doc` (`None` for an entry that is not one
/// of its pages), or `None` when the page is not a separation.
fn members(doc: &Document, index: usize) -> Option<Vec<Option<usize>>> {
    let all: Vec<ObjRef> = pages(doc).unwrap().into_iter().map(|p| p.obj).collect();
    let list = separation(doc, index)?.get(b"Pages").map(|p| doc.resolve(p).as_array().cloned().unwrap_or_default())?;
    Some(list.iter().map(|e| e.as_ref().and_then(|r| all.iter().position(|p| *p == r))).collect())
}

/// How many page objects the saved document holds, listed in its page tree or not.
fn page_objects(doc: &Document) -> usize {
    doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Page"))).count()
}

#[test]
fn deleting_a_plate_leaves_the_others_listing_only_each_other() {
    for shared in [false, true] {
        let mut doc = plates(shared);
        delete_pages(&mut doc, &[1, 4]).unwrap();
        let out = full_roundtrip(&doc);
        for page in 0..3 {
            assert_eq!(members(&out, page), Some(vec![Some(0), Some(1), Some(2)]), "page {page}, shared {shared}");
        }
        // The rest of the dictionary stays.
        let info = separation(&out, 0).unwrap();
        assert!(info.get(b"DeviceColorant").is_some() && info.get(b"ColorSpace").is_some());
    }
}

#[test]
fn extracting_a_plate_brings_no_other_plate_along() {
    for shared in [false, true] {
        let out = full_roundtrip(&extract_pages(&plates(shared), &[0]).unwrap());
        assert_eq!(members(&out, 0), Some(vec![Some(0)]), "shared {shared}");
        assert_eq!(page_objects(&out), 1, "no copy of the other plates, shared {shared}");
        let info = separation(&out, 0).unwrap();
        let space = out.resolve(info.get(b"ColorSpace").unwrap()).as_array().cloned().unwrap();
        let tint = space.get(3).map(|f| out.resolve(f)).and_then(|f| f.as_dict().and_then(|d| d.get(b"FunctionType")).cloned());
        assert_eq!(tint, Some(Object::Int(2)), "the tint transform comes along");
    }
}

#[test]
fn extracting_and_splitting_list_only_the_plates_that_came_along() {
    let out = full_roundtrip(&extract_pages(&plates(false), &[2, 0]).unwrap());
    // In the set's own order: Cyan (now page 1), then Yellow (page 0).
    for page in 0..2 {
        assert_eq!(members(&out, page), Some(vec![Some(1), Some(0)]), "page {page}");
    }
    assert_eq!(page_objects(&out), 2);
    let parts = split(&plates(true), &SplitBy::PageCount(2)).unwrap();
    let parts: Vec<Document> = parts.iter().map(full_roundtrip).collect();
    for part in &parts[..2] {
        for page in 0..2 {
            assert_eq!(members(part, page), Some(vec![Some(0), Some(1)]));
        }
        assert_eq!(page_objects(part), 2);
    }
    assert_eq!(members(&parts[2], 0), None);
}

#[test]
fn combining_keeps_each_file_s_set_to_its_own_copies() {
    let (a, b) = (plates(false), plates(true));
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &b)]).unwrap());
    assert_eq!(page_count(&out).unwrap(), 10);
    assert_eq!(page_objects(&out), 10);
    for page in 0..4 {
        assert_eq!(members(&out, page), Some((0..4).map(Some).collect()), "page {page}");
        assert_eq!(members(&out, page + 5), Some((5..9).map(Some).collect()), "page {}", page + 5);
    }
}

#[test]
fn a_copied_plate_is_a_set_of_its_own() {
    // Duplicating a plate does not add it to the original set, whose other pages don't list it.
    let mut doc = plates(false);
    duplicate_pages(&mut doc, &[0], Some(5)).unwrap();
    let out = full_roundtrip(&doc);
    assert_eq!(members(&out, 5), Some(vec![Some(5)]));
    for page in 0..4 {
        assert_eq!(members(&out, page), Some((0..4).map(Some).collect()), "page {page}");
    }
    assert_eq!(page_objects(&out), 6);
    // A plate extracted twice: the second copy lists only itself, not the first.
    let out = full_roundtrip(&extract_pages(&plates(true), &[0, 0]).unwrap());
    assert_eq!(members(&out, 0), Some(vec![Some(0)]));
    assert_eq!(members(&out, 1), Some(vec![Some(1)]));
}
