use pdfcraft_cos::{Lexer, Object};

use super::*;

fn parse(text: &[u8]) -> Object {
    Lexer::new(text, 0).object().unwrap()
}

/// A Redact dictionary with these `/QuadPoints` and `/Rect` (either may be absent).
fn mark(quads: Option<&[u8]>, rect: Option<&[u8]>) -> Dict {
    let mut d = Dict::default();
    d.set(b"Type".to_vec(), Object::Name(b"Annot".to_vec()));
    d.set(b"Subtype".to_vec(), Object::Name(b"Redact".to_vec()));
    if let Some(q) = quads {
        d.set(b"QuadPoints".to_vec(), parse(q));
    }
    if let Some(r) = rect {
        d.set(b"Rect".to_vec(), parse(r));
    }
    d
}

fn areas(quads: Option<&[u8]>, rect: Option<&[u8]>) -> Result<MarkAreas, MarkAreaError> {
    mark_areas(&Document::new_empty(), &mark(quads, rect))
}

const RECT: &[u8] = b"[20 95 40 110]";
const AB: [f64; 4] = [20.0, 95.0, 40.0, 110.0];

#[test]
fn whole_valid_quads_are_the_areas_and_rect_is_not_read() {
    let a = areas(Some(b"[20 110 25 110 20 95 25 95 30 110 40 110 30 95 40 95]"), Some(b"[null]")).unwrap();
    assert_eq!(a.rects, vec![[20.0, 95.0, 25.0, 110.0], [30.0, 95.0, 40.0, 110.0]], "two lines, never their union");
    assert!(!a.used_rect && !a.widened);
    assert_eq!(a.quad_issues, QuadIssues::default());
}

/// The reported case: a null first corner. Not `[0 95 40 110]`, which reading null as 0 gave.
#[test]
fn a_null_corner_brings_in_the_rect() {
    let a = areas(Some(b"[null 110 40 110 30 95 40 95]"), Some(RECT)).unwrap();
    assert_eq!(a.rects, vec![AB]);
    assert!(a.used_rect);
    assert_eq!(a.quad_issues.invalid_groups, 1);
    assert_eq!(areas(Some(b"[null 110 40 110 30 95 40 95]"), None), Err(MarkAreaError::NoUsableArea));
}

#[test]
fn every_bad_corner_position_and_value_makes_the_quad_unusable() {
    let good = [30.0, 110.0, 40.0, 110.0, 30.0, 95.0, 40.0, 95.0];
    for i in 0..8 {
        for bad in [&b"null"[..], b"-nan", b"true", b"(1)", b"/N", b"[1]"] {
            let text: Vec<String> =
                good.iter().enumerate().map(|(j, v)| if i == j { String::from_utf8_lossy(bad).into_owned() } else { v.to_string() }).collect();
            let q = format!("[{}]", text.join(" "));
            let a = areas(Some(q.as_bytes()), Some(RECT)).unwrap();
            assert_eq!((a.rects.as_slice(), a.quad_issues.invalid_groups), (&[AB][..], 1), "{q}");
        }
    }
    // Programmatic infinities, which the parser can't produce.
    let mut d = mark(None, Some(RECT));
    let mut q: Vec<Object> = good.iter().map(|v| Object::Real(*v)).collect();
    q[5] = Object::Real(f64::NEG_INFINITY);
    d.set(b"QuadPoints".to_vec(), Object::Array(q));
    assert_eq!(mark_areas(&Document::new_empty(), &d).unwrap().rects, vec![AB]);
}

#[test]
fn mixed_quads_keep_the_usable_ones_and_add_the_rect() {
    // A usable quad outside /Rect stays, in its order, with /Rect appended last.
    let a = areas(Some(b"[100 110 120 110 100 95 120 95 -nan 110 40 110 30 95 40 95]"), Some(RECT)).unwrap();
    assert_eq!(a.rects, vec![[100.0, 95.0, 120.0, 110.0], AB]);
    assert!(a.used_rect && a.quad_issues.partial());
    // Without /Rect the whole mark is refused, not its usable part applied.
    assert_eq!(areas(Some(b"[100 110 120 110 100 95 120 95 -nan 110 40 110 30 95 40 95]"), None), Err(MarkAreaError::QuadNeedsRect));
    // An invalid group first does not move the next one's corners.
    let a = areas(Some(b"[null 1 2 3 4 5 6 7 100 110 120 110 100 95 120 95]"), Some(RECT)).unwrap();
    assert_eq!(a.rects, vec![[100.0, 95.0, 120.0, 110.0], AB]);
}

#[test]
fn trailing_entries_bring_in_the_rect() {
    for tail in 1..=7usize {
        let extra: Vec<&str> = std::iter::repeat_n("5", tail).collect();
        let q = format!("[20 110 25 110 20 95 25 95 {}]", extra.join(" "));
        let a = areas(Some(q.as_bytes()), Some(RECT)).unwrap();
        assert_eq!(a.rects, vec![[20.0, 95.0, 25.0, 110.0], AB], "tail {tail}");
        assert_eq!(a.quad_issues.trailing_values, tail);
        assert_eq!(areas(Some(q.as_bytes()), None), Err(MarkAreaError::QuadNeedsRect), "tail {tail}");
    }
    // Only a tail: no usable quad, so /Rect alone (or refusal).
    assert_eq!(areas(Some(b"[1 2 3]"), Some(RECT)).unwrap().rects, vec![AB]);
    assert_eq!(areas(Some(b"[1 2 3]"), None), Err(MarkAreaError::NoUsableArea));
}

#[test]
fn absent_empty_or_malformed_quadpoints_use_the_rect() {
    for (q, absent, malformed) in [(None, true, false), (Some(&b"[]"[..]), true, false), (Some(b"/Wrong"), false, true), (Some(b"12"), false, true)] {
        let a = areas(q, Some(RECT)).unwrap();
        assert_eq!(a.rects, vec![AB]);
        assert_eq!((a.quad_issues.absent_or_empty, a.quad_issues.malformed_container), (absent, malformed));
        assert_eq!(areas(q, None), Err(MarkAreaError::NoUsableArea));
    }
}

#[test]
fn a_rect_must_have_area_and_four_finite_numbers() {
    for bad in [&b"[20 95 20 110]"[..], b"[20 95 40 95]", b"[20 95 40]", b"[null 20 95 40 110]", b"[20 95 40 -nan]", b"/R"] {
        assert_eq!(areas(None, Some(bad)), Err(MarkAreaError::NoUsableArea), "{}", String::from_utf8_lossy(bad));
    }
    assert_eq!(areas(None, Some(b"[40 110 20 95]")).unwrap().rects, vec![AB], "reversed corners are ordered");
}

#[test]
fn degenerate_quads_are_unusable() {
    // A zero-width quad (all x equal) counts as invalid: it brings in /Rect.
    let a = areas(Some(b"[20 110 20 110 20 95 20 95]"), Some(RECT)).unwrap();
    assert_eq!((a.rects, a.quad_issues.invalid_groups), (vec![AB], 1));
    assert_eq!(areas(Some(b"[20 110 20 110 20 95 20 95]"), None), Err(MarkAreaError::NoUsableArea));
}

#[test]
fn over_the_cap_is_refused_even_with_a_rect() {
    let mut d = mark(None, Some(RECT));
    d.set(b"QuadPoints".to_vec(), Object::Array(vec![Object::Int(1); MAX_QUADPOINT_VALUES + 8]));
    assert_eq!(mark_areas(&Document::new_empty(), &d), Err(MarkAreaError::TooManyValues));
    // At the cap is read (all degenerate here, so /Rect is used).
    d.set(b"QuadPoints".to_vec(), Object::Array(vec![Object::Int(1); MAX_QUADPOINT_VALUES]));
    let a = mark_areas(&Document::new_empty(), &d).unwrap();
    assert_eq!((a.rects, a.quad_issues.invalid_groups), (vec![AB], MAX_QUADPOINT_VALUES / 8));
}

#[test]
fn indirect_arrays_entries_and_chains_are_followed() {
    let mut doc = Document::new_empty();
    // The held regression (44a230dd): an indirect corner keeps its quad's own box.
    let corner = doc.add(Object::Int(20));
    let mut q = vec![Object::Ref(corner)];
    q.extend([110, 40, 110, 20, 95, 40, 95].map(Object::Int));
    let mut d = mark(None, None);
    d.set(b"QuadPoints".to_vec(), Object::Array(q.clone()));
    assert_eq!(mark_areas(&doc, &d).unwrap().rects, vec![AB]);
    // A chain to the corner, and to the array itself.
    let c1 = doc.add(Object::Ref(corner));
    let c2 = doc.add(Object::Ref(c1));
    let mut q2 = q.clone();
    q2[0] = Object::Ref(c2);
    let arr = doc.add(Object::Array(q2));
    let arr = doc.add(Object::Ref(arr));
    d.set(b"QuadPoints".to_vec(), Object::Ref(arr));
    assert_eq!(mark_areas(&doc, &d).unwrap().rects, vec![AB]);
    // Indirect /Rect entries.
    let x = doc.add(Object::Int(40));
    let mut d = mark(Some(b"[null 1 2 3 4 5 6 7]"), None);
    d.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(20), Object::Int(95), Object::Ref(x), Object::Int(110)]));
    assert_eq!(mark_areas(&doc, &d).unwrap().rects, vec![AB]);
    // A cycle in the QuadPoints reference is a malformed container: /Rect, or refusal.
    let me = doc.add(Object::Null);
    doc.set(me, Object::Ref(me));
    let mut d = mark(None, Some(RECT));
    d.set(b"QuadPoints".to_vec(), Object::Ref(me));
    let a = mark_areas(&doc, &d).unwrap();
    assert!(a.quad_issues.malformed_container);
    assert_eq!(a.rects, vec![AB]);
    // A cycle in a corner is an invalid group.
    let mut d = mark(None, None);
    let mut q3 = q;
    q3[0] = Object::Ref(me);
    d.set(b"QuadPoints".to_vec(), Object::Array(q3));
    assert_eq!(mark_areas(&doc, &d), Err(MarkAreaError::NoUsableArea));
}

#[test]
fn areas_are_exact_f32_rectangles_containing_the_source() {
    let a = areas(Some(b"[0.1 0.7 0.3 0.7 0.1 0.2 0.3 0.2]"), None).unwrap();
    let r = a.rects[0];
    assert!(a.widened);
    assert!(r[0] <= 0.1 && r[1] <= 0.2 && r[2] >= 0.3 && r[3] >= 0.7, "{r:?}");
    assert!(r.iter().all(|v| f64::from(*v as f32) == *v), "{r:?} is exactly f32");
    // Too large for f32: refused, never narrowed to /Rect or clamped.
    assert_eq!(areas(Some(b"[0 1 1e39 1 0 0 1e39 0]"), Some(RECT)), Err(MarkAreaError::UnrepresentableGeometry));
    assert_eq!(areas(None, Some(b"[0 0 1e300 1]")), Err(MarkAreaError::UnrepresentableGeometry));
    // A quad whose width overflows f64 has no finite area: unusable, so /Rect.
    assert_eq!(areas(Some(b"[-1.7e308 1 1.7e308 1 -1.7e308 0 1.7e308 0]"), Some(RECT)).unwrap().rects, vec![AB]);
}

#[test]
fn the_locator_is_a_safe_rect_or_nothing() {
    let doc = Document::new_empty();
    assert_eq!(mark_locator(&doc, &mark(None, Some(RECT))), Some(AB));
    assert_eq!(mark_locator(&doc, &mark(None, Some(b"[null 95 40 110]"))), None, "never a zero substituted for null");
    assert_eq!(mark_locator(&doc, &mark(None, Some(b"[20 95 20 110]"))), None);
    assert_eq!(mark_locator(&doc, &mark(None, None)), None);
}
