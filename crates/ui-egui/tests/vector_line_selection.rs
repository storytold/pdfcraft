//! Regression tests for vector line selection primitives.

use pdfcraft_edit::LineSegment;

#[test]
fn line_bbox_and_length_are_consistent() {
    let l = LineSegment { page: 0, index: 0, start: [0.0, 0.0], end: [3.0, 4.0], thickness: 1.0 };
    assert_eq!(l.bbox(), [0.0, 0.0, 3.0, 4.0]);
    assert!((l.length() - 5.0).abs() < 1e-9);
}

#[test]
fn distance_sq_to_handles_degenerate_segment() {
    let l = LineSegment { page: 0, index: 0, start: [2.0, 2.0], end: [2.0, 2.0], thickness: 1.0 };
    assert!((l.distance_sq_to([2.0, 2.0])).abs() < 1e-9);
    assert!((l.distance_sq_to([5.0, 2.0]) - 9.0).abs() < 1e-9);
}

#[test]
#[ignore = "needs PageXform fixture"]
fn line_endpoint_handles_returns_two_handles() {
    // Assert line_endpoint_handles returns exactly two entries and the
    // LineStart/LineEnd handles are at the projected endpoints.
}

#[test]
#[ignore = "needs PageXform fixture"]
fn hit_test_line_endpoint_finds_start_and_end() {
    // Assert that a pos at the projected start hits LineStart,
    // a pos at the projected end hits LineEnd, and a pos 100 px away hits None.
}
