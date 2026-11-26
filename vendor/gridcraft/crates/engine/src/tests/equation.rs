//! Equations: insert, edit, the layout cache and its rebuild on load.

use gridcraft_model::{Color, Shape, ShapeKind};
use serde_json::json;

use super::s;
use crate::Session;

fn equation(s: &Session, id: u32) -> Shape {
    s.doc().unwrap().wb.active().unwrap().shapes.iter().find(|x| x.id == id).cloned().unwrap()
}

fn insert(s: &mut Session, p: serde_json::Value) -> u32 {
    s.execute("insert.equation", p).unwrap()["equation"].as_u64().unwrap() as u32
}

#[test]
fn insert_lays_the_equation_out_and_sizes_its_frame() {
    let mut s = s();
    let id = insert(&mut s, json!({"latex": "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}", "cell": "C3"}));
    let e = equation(&s, id);
    assert_eq!(e.kind, ShapeKind::Equation);
    assert_eq!(e.anchor.cell.a1(), "C3");
    assert_eq!(e.line, Color::Auto, "Automatic ink");
    let list = e.math.get(&e.text).unwrap().unwrap();
    let scale = crate::cmd::equation::fit_scale(list, e.anchor.width, e.anchor.height);
    assert!((scale - 11.0 * 96.0 / 72.0).abs() < 0.01, "11 pt: {scale}");
    // A bigger size makes a bigger frame; `at` works like `cell`.
    let big = insert(&mut s, json!({"latex": "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}", "at": "H3", "size": 22, "color": "#1F4E79"}));
    let b = equation(&s, big);
    assert!((b.anchor.width - 8.0) > 1.9 * (e.anchor.width - 8.0));
    assert_eq!(b.line, Color::Rgb(0x1F4E79));
    assert_eq!(b.anchor.cell.a1(), "H3");
    // One undo step each.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().shapes.iter().all(|x| x.id != big));
}

#[test]
fn insert_refuses_missing_bad_or_hostile_source() {
    let mut s = s();
    for p in [
        json!({}),
        json!({"latex": ""}),
        json!({"latex": "   "}),
        json!({"latex": 5}),
        json!({"latex": "x", "size": 0}),
        json!({"latex": "x", "size": f64::MAX}),
        json!({"latex": "x", "color": "nope"}),
    ] {
        assert!(s.execute("insert.equation", p.clone()).is_err(), "{p}");
    }
    let e = s.execute("insert.equation", json!({"latex": "a + \\foo"})).unwrap_err().to_string();
    assert!(e.contains("character 5") && e.contains("\\foo"), "{e}");
    let e = s.execute("insert.equation", json!({"latex": "x".repeat(5000)})).unwrap_err().to_string();
    assert!(e.contains("4096"), "{e}");
    let e = s.execute("insert.equation", json!({"latex": format!("{}x{}", "{".repeat(40), "}".repeat(40))})).unwrap_err().to_string();
    assert!(e.contains("nest"), "{e}");
    assert!(s.doc().unwrap().wb.active().unwrap().shapes.is_empty());
    // `equation.edit` is the other way a source reaches the layout engine; it has the same caps,
    // and a refused edit leaves the equation as it was.
    let id = insert(&mut s, json!({"latex": "x"}));
    let e = s.execute("equation.edit", json!({"id": id, "latex": "x".repeat(5000)})).unwrap_err().to_string();
    assert!(e.contains("4096"), "{e}");
    let e = s.execute("equation.edit", json!({"id": id, "latex": format!("{}x{}", "{".repeat(40), "}".repeat(40))})).unwrap_err().to_string();
    assert!(e.contains("nest"), "{e}");
    assert_eq!(equation(&s, id).text, "x");
}

#[test]
fn edit_keeps_the_scale_and_fits_the_frame() {
    let mut s = s();
    let id = insert(&mut s, json!({"latex": "x", "size": 20}));
    let before = equation(&s, id);
    s.execute("equation.edit", json!({"id": id, "latex": "x + y + z"})).unwrap();
    let after = equation(&s, id);
    assert_eq!(after.text, "x + y + z");
    let list = after.math.get(&after.text).unwrap().unwrap();
    let scale = crate::cmd::equation::fit_scale(list, after.anchor.width, after.anchor.height);
    assert!((scale - 20.0 * 96.0 / 72.0).abs() < 0.05, "{scale}");
    assert!(after.anchor.width > before.anchor.width);
    // A frame the user resized keeps its new scale through an edit.
    s.execute("object.move", json!({"kind": "equation", "id": id, "width": after.anchor.width * 2.0, "height": after.anchor.height * 2.0})).unwrap();
    s.execute("equation.edit", json!({"id": id, "latex": "x + y"})).unwrap();
    let e = equation(&s, id);
    let scale = crate::cmd::equation::fit_scale(e.math.get(&e.text).unwrap().unwrap(), e.anchor.width, e.anchor.height);
    assert!(scale > 2.0 * 20.0 * 96.0 / 72.0 * 0.95, "{scale}");
    // Undo restores the source and its layout.
    s.execute("edit.undo", json!({})).unwrap();
    let e = equation(&s, id);
    assert_eq!(e.text, "x + y + z");
    assert!(e.math.get(&e.text).unwrap().is_ok());
    // Same source: nothing to do. Bad source: refused, nothing changes.
    let depth = s.doc().unwrap().undo.len();
    s.execute("equation.edit", json!({"id": id, "latex": "x + y + z"})).unwrap();
    assert_eq!(s.doc().unwrap().undo.len(), depth);
    assert!(s.execute("equation.edit", json!({"id": id, "latex": "\\frac{"})).is_err());
    assert!(s.execute("equation.edit", json!({"id": id, "latex": ""})).is_err());
    assert_eq!(equation(&s, id).text, "x + y + z");
}

#[test]
fn the_source_excerpt_is_at_most_the_cap_and_whole_chars() {
    use crate::cmd::equation::source_excerpt;
    use gridcraft_math::MAX_SOURCE_BYTES as CAP;
    assert_eq!(source_excerpt(""), "");
    assert_eq!(source_excerpt("x+y"), "x+y");
    let at = "x".repeat(CAP);
    assert_eq!(source_excerpt(&at), at, "a source at the cap is whole");
    assert_eq!(source_excerpt(&format!("{at}tail")), at);
    // A three-byte character straddling the cap is left out rather than cut in two.
    let s = format!("{}\u{20AC}", "x".repeat(CAP - 1));
    assert_eq!(source_excerpt(&s), "x".repeat(CAP - 1));
    let s = format!("{}\u{20AC}\u{20AC}", "x".repeat(CAP - 2));
    assert_eq!(source_excerpt(&s).len(), CAP - 2);
    assert_eq!(source_excerpt(&"\u{1F600}".repeat(2000)).len(), CAP);
}

#[test]
fn edit_refuses_what_is_not_an_equation() {
    let mut s = s();
    let tb = s.execute("insert.textBox", json!({"text": "hi"})).unwrap()["shape"].as_u64().unwrap();
    assert!(s.execute("equation.edit", json!({"id": tb, "latex": "x"})).is_err());
    assert!(s.execute("equation.edit", json!({"id": 999, "latex": "x"})).is_err());
    assert!(s.execute("equation.edit", json!({"latex": "x"})).is_err());
    assert!(s.execute("equation.edit", json!({"id": -1, "latex": "x"})).is_err());
}

#[test]
fn layouts_are_rebuilt_on_load_and_not_saved() {
    let mut s = s();
    let id = insert(&mut s, json!({"latex": "e^{i\\pi} + 1 = 0"}));
    let wb = s.doc().unwrap().wb.clone();
    let bytes = serde_json::to_vec(&*wb).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("\"math\""), "the cache is not saved");
    let (opened, _) = crate::io::open_bytes("book.json", &bytes).unwrap();
    let raw = opened.active().unwrap().shapes.iter().find(|x| x.id == id).cloned().unwrap();
    assert!(raw.math.get(&raw.text).is_none(), "a freshly read shape has no layout");
    let mut t = Session::new();
    t.add_document(crate::DocState::new(opened, None, "Book".into(), crate::Trust::FromFile));
    let e = equation(&t, id);
    assert!(e.math.get(&e.text).unwrap().is_ok(), "opening lays it out");
}

#[test]
fn a_bad_source_from_a_file_keeps_its_error() {
    let mut s = s();
    let id = insert(&mut s, json!({"latex": "x"}));
    let mut wb = (*s.doc().unwrap().wb).clone();
    if let Some(sh) = wb.sheet_mut(0) {
        sh.shapes.iter_mut().for_each(|x| x.text = "\\frac{".into());
    }
    let mut t = Session::new();
    t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
    let e = equation(&t, id);
    assert!(e.math.get(&e.text).unwrap().is_err());
}

#[test]
fn equations_on_a_protected_sheet_are_locked() {
    let mut s = s();
    let id = insert(&mut s, json!({"latex": "x"}));
    s.execute("review.protectSheet", json!({})).unwrap();
    assert!(s.execute("equation.edit", json!({"id": id, "latex": "y"})).is_err());
    assert!(s.execute("insert.equation", json!({"latex": "y"})).is_err());
}

#[test]
fn a_file_with_too_many_equations_lays_out_a_bounded_amount() {
    // About 1 KB each: past the budget, equations are refused without running the layout engine,
    // so opening the file takes bounded time.
    let source = format!("x{}", "+x".repeat(500));
    let count = gridcraft_model::MAX_EQUATION_LAYOUT_BYTES / source.len() + 40;
    let mut s = s();
    let first = insert(&mut s, json!({"latex": "x", "cell": "A1"}));
    let mut wb = (*s.doc().unwrap().wb).clone();
    if let Some(sh) = wb.sheet_mut(0) {
        let template = sh.shapes.iter().find(|x| x.id == first).cloned().unwrap();
        sh.shapes.clear();
        for i in 0..count {
            let mut e = template.clone();
            e.id = 100 + i as u32;
            e.text = source.clone();
            e.math = Default::default();
            sh.shapes.push(e);
        }
    }
    let mut t = Session::new();
    t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
    let shapes = t.doc().unwrap().wb.active().unwrap().shapes.clone();
    let laid = shapes.iter().filter(|x| matches!(x.math.get(&x.text), Some(Ok(_)))).count();
    let refused: Vec<&Shape> =
        shapes.iter().filter(|x| matches!(x.math.get(&x.text), Some(Err(e)) if e.message == gridcraft_model::EQUATION_BUDGET_MESSAGE)).collect();
    assert_eq!(laid + refused.len(), count, "every equation has a layout or the budget's error");
    assert!(laid * source.len() <= gridcraft_model::MAX_EQUATION_LAYOUT_BYTES, "{laid} laid out");
    assert!(refused.len() >= 40, "{} refused", refused.len());
    // A later edit does not retry them all, and OK on one, unchanged, lays it out.
    let id = refused[0].id;
    t.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    assert!(matches!(equation(&t, id).math.get(&source), Some(Err(_))));
    t.execute("equation.edit", json!({"id": id, "latex": source})).unwrap();
    assert!(matches!(equation(&t, id).math.get(&source), Some(Ok(_))), "editing lays it out");
}

#[test]
fn ids_stay_unique_when_a_file_used_the_largest_id() {
    // A workbook (from a JSON file, say) whose shape has id u32::MAX: new equations and the
    // objects of a copied sheet still get ids no other object has.
    let mut s = s();
    let first = insert(&mut s, json!({"latex": "x", "cell": "A1"}));
    let mut wb = (*s.doc().unwrap().wb).clone();
    if let Some(sh) = wb.sheet_mut(0) {
        sh.shapes.iter_mut().filter(|x| x.id == first).for_each(|x| x.id = u32::MAX);
    }
    let mut t = Session::new();
    t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
    let a = insert(&mut t, json!({"latex": "y", "cell": "C1"}));
    let b = insert(&mut t, json!({"latex": "z", "cell": "E1"}));
    t.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    let wb = &t.doc().unwrap().wb;
    let mut ids: Vec<u32> = wb.sheets.iter().flat_map(|sh| sh.shapes.iter().map(|x| x.id)).collect();
    assert_eq!(ids.len(), 6, "three equations on each sheet");
    assert!(a != u32::MAX && b != u32::MAX && a != b, "{a} {b}");
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 6, "every id is distinct: {ids:?}");
}
