//! Regression tests for vector line selection primitives.

use egui::{Pos2, Rect, pos2, vec2};
use pdfcraft_edit::LineSegment;
use pdfcraft_render::{DocInfo, PageInfo};
use pdfcraft_ui_egui::canvas::PageXform;
use pdfcraft_ui_egui::canvas_selection::{SelectionHandle, hit_test_line_endpoint, line_endpoint_handles};

#[test]
fn line_bbox_and_length_are_consistent() {
    let l = LineSegment { page: 0, index: 0, start: [0.0, 0.0], end: [3.0, 4.0], thickness: 1.0, stroke_color: None };
    assert_eq!(l.bbox(), [0.0, 0.0, 3.0, 4.0]);
    assert!((l.length() - 5.0).abs() < 1e-9);
}

#[test]
fn distance_sq_to_handles_degenerate_segment() {
    let l = LineSegment { page: 0, index: 0, start: [2.0, 2.0], end: [2.0, 2.0], thickness: 1.0, stroke_color: None };
    assert!((l.distance_sq_to([2.0, 2.0])).abs() < 1e-9);
    assert!((l.distance_sq_to([5.0, 2.0]) - 9.0).abs() < 1e-9);
}

#[test]
fn line_endpoint_handles_returns_two_handles() {
    let page_info = PageInfo { width: 612.0, height: 792.0, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 };
    let info = DocInfo { pages: vec![page_info], ..Default::default() };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(612.0, 792.0)), rot: 0, pw: 612.0, ph: 792.0 };
    let line = LineSegment { page: 0, index: 0, start: [100.0, 200.0], end: [300.0, 400.0], thickness: 1.0, stroke_color: None };
    let handles = line_endpoint_handles(&line, &xf, &info, 8.0);
    assert_eq!(handles.len(), 2);
    assert_eq!(handles[0].0, SelectionHandle::LineStart);
    assert_eq!(handles[1].0, SelectionHandle::LineEnd);

    assert!((handles[0].1.center().x - 100.0).abs() < 1e-3);
    assert!((handles[0].1.center().y - 592.0).abs() < 1e-3);
    assert!((handles[1].1.center().x - 300.0).abs() < 1e-3);
    assert!((handles[1].1.center().y - 392.0).abs() < 1e-3);
}

#[test]
fn hit_test_line_endpoint_finds_start_and_end() {
    let page_info = PageInfo { width: 612.0, height: 792.0, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 };
    let info = DocInfo { pages: vec![page_info], ..Default::default() };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(612.0, 792.0)), rot: 0, pw: 612.0, ph: 792.0 };
    let line = LineSegment { page: 0, index: 0, start: [100.0, 200.0], end: [300.0, 400.0], thickness: 1.0, stroke_color: None };

    let hit_start = hit_test_line_endpoint(&line, pos2(100.0, 592.0), &xf, &info, 8.0);
    assert_eq!(hit_start, Some(SelectionHandle::LineStart));

    let hit_end = hit_test_line_endpoint(&line, pos2(300.0, 392.0), &xf, &info, 8.0);
    assert_eq!(hit_end, Some(SelectionHandle::LineEnd));

    let hit_miss = hit_test_line_endpoint(&line, pos2(0.0, 0.0), &xf, &info, 8.0);
    assert_eq!(hit_miss, None);
}
