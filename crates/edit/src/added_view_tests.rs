//! #7248: added text keeps where it is drawn when its page is rotated or cropped after it was
//! added and it is restyled afterwards. Its box is listed as the page shows it now, and a restyle
//! draws it again for the view it was written for.

use super::{fixture, reopen};
use crate::*;
use pdfcraft_content::Matrix;
use pdfcraft_cos::{Document, Object};

/// The added item's stream's own matrix (`q … cm`) and its stored box, in user space.
fn drawn_at(doc: &Document, page: usize) -> [f64; 4] {
    let a = list_added(doc).into_iter().find(|a| a.page == page).expect("an added item");
    let Object::Stream(s) = &*doc.get(a.obj) else { panic!("a stream") };
    let data = s.decoded_within(1 << 20).unwrap();
    let words: Vec<&[u8]> = data.split(u8::is_ascii_whitespace).filter(|w| !w.is_empty()).take(8).collect();
    assert_eq!((words[0], words[7]), (&b"q"[..], &b"cm"[..]));
    let m: Vec<f64> = words[1..7].iter().map(|w| std::str::from_utf8(w).unwrap().parse().unwrap()).collect();
    let params = s.dict.get(b"PCAdded").and_then(Object::as_dict).unwrap();
    let r: Vec<f64> = params.get(b"Rect").and_then(Object::as_array).unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    Matrix(m.try_into().unwrap()).bbox(r.try_into().unwrap())
}

fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

fn set_on_page(doc: &mut Document, page: usize, key: &[u8], value: Object) {
    let obj = pdfcraft_model::pages(doc)[page].obj;
    doc.update_dict(obj, |d| d.set(key.to_vec(), value)).unwrap();
}

/// Adds a word to page 3 (upright, no crop), changes the page with `change`, then recolours it.
fn restyled_after(change: impl FnOnce(&mut Document)) -> (Document, [f64; 4], [f64; 4]) {
    let mut doc = fixture();
    let text = AddedText { rect: [50.0, 700.0, 250.0, 750.0], text: "POSITION".into(), size: 24.0, ..AddedText::default() };
    add_content(&mut doc, 2, &Content::Text(text)).unwrap();
    change(&mut doc);
    let mut doc = reopen(&doc);
    let before = drawn_at(&doc, 2);
    let listed = list_added(&doc).into_iter().find(|a| a.page == 2).unwrap().content;
    let Content::Text(mut t) = listed.clone() else { panic!("text") };
    t.color = [1.0, 0.0, 0.0];
    update_content(&mut doc, 2, 0, &Content::Text(t)).unwrap();
    let after = drawn_at(&doc, 2);
    let listed_after = list_added(&doc).into_iter().find(|a| a.page == 2).unwrap().content.rect();
    assert!(close(listed.rect(), listed_after), "listed box before {:?}, after {listed_after:?}", listed.rect());
    (doc, before, after)
}

#[test]
fn a_restyle_after_the_page_is_rotated_keeps_the_text_where_it_is() {
    let (doc, before, after) = restyled_after(|doc| set_on_page(doc, 2, b"Rotate", Object::Int(90)));
    assert!(close(before, after), "drawn at {before:?}, then at {after:?}");
    // Listed as the turned page shows it: inside the 800 × 600 displayed page.
    let shown = list_added(&doc).into_iter().find(|a| a.page == 2).unwrap().content.rect();
    assert!(shown.iter().all(|v| (0.0..=800.0).contains(v)), "{shown:?}");
}

#[test]
fn a_restyle_after_the_page_is_cropped_keeps_the_text_where_it_is() {
    let crop = Object::Array([30.0, 40.0, 590.0, 780.0].map(Object::Real).to_vec());
    let (_, before, after) = restyled_after(|doc| set_on_page(doc, 2, b"CropBox", crop));
    assert!(close(before, after), "drawn at {before:?}, then at {after:?}");
}

#[test]
fn a_restyle_on_an_unchanged_page_is_as_before() {
    let (_, before, after) = restyled_after(|_| {});
    assert!(close(before, after), "drawn at {before:?}, then at {after:?}");
}

/// CodeRabbit on #7278: under a large /UserUnit a crop moved by a fraction of a user unit is a
/// move of whole units on the page, so the item is listed where the page now shows it: a CropBox
/// moved 0.0005 to the right under a /UserUnit of 10000 moves it 5 units to the left.
#[test]
fn a_tiny_crop_under_a_large_user_unit_moves_the_listed_box() {
    let mut doc = fixture();
    set_on_page(&mut doc, 2, b"UserUnit", Object::Real(10_000.0));
    let text = AddedText { rect: [50.0, 700.0, 250.0, 750.0], text: "POSITION".into(), size: 24.0, ..AddedText::default() };
    add_content(&mut doc, 2, &Content::Text(text)).unwrap();
    let listed = |doc: &Document| list_added(doc).into_iter().find(|a| a.page == 2).unwrap().content.rect();
    let doc = reopen(&doc);
    let before = listed(&doc);
    let mut doc = doc;
    let media = pdfcraft_model::pages(&doc)[2].crop(&doc);
    let crop = Object::Array([media[0] + 0.0005, media[1], media[2], media[3]].map(Object::Real).to_vec());
    set_on_page(&mut doc, 2, b"CropBox", crop);
    let after = listed(&reopen(&doc));
    assert!((before[0] - after[0] - 5.0).abs() < 0.01 && (before[2] - after[2] - 5.0).abs() < 0.01, "listed at {before:?}, then at {after:?}");
}
