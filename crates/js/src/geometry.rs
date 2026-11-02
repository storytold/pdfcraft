//! Conversion between PDF default user space and Acrobat's widget rectangle convention.
pub fn rotate_rect([x0, y0, x1, y1]: [f64; 4], [cx0, cy0, cx1, cy1]: [f64; 4], rotation: i64) -> [f64; 4] {
    match rotation.rem_euclid(360) {
        90 => [y0 - cy0, cx1 - x0, y1 - cy0, cx1 - x1],
        180 => [cx1 - x1, cy1 - y0, cx1 - x0, cy1 - y1],
        270 => [cy1 - y1, x1 - cx0, cy1 - y0, x0 - cx0],
        _ => [x0, y1, x1, y0],
    }
}
pub fn unrotate_rect([l, t, r, b]: [f64; 4], [cx0, cy0, cx1, cy1]: [f64; 4], rotation: i64) -> [f64; 4] {
    match rotation.rem_euclid(360) {
        90 => [cx1 - t, cy0 + l, cx1 - b, cy0 + r],
        180 => [cx1 - r, cy1 - t, cx1 - l, cy1 - b],
        270 => [cx0 + b, cy1 - r, cx0 + t, cy1 - l],
        _ => [l, b, r, t],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotated_widget_rectangles_roundtrip_with_shifted_crop_boxes() {
        let rect = [30.0, 50.0, 90.0, 100.0];
        let crop = [20.0, 40.0, 620.0, 840.0];
        for rotation in [0, 90, 180, 270] {
            assert_eq!(unrotate_rect(rotate_rect(rect, crop, rotation), crop, rotation), rect);
        }
    }
}
