//! Fill & Sign geometry: typed signatures, and boxes placed as a turned page displays them.

use crate::{Shape, script_outline, signature_image};

/// Fill & Sign: `text` in the script font as a typed signature, its left edge at `at` (user
/// space, vertically centred) and `height` points tall. `None` for text with no outlines.
///
/// Left, centred and upright are as displayed on a page turned by `rotation` (its `/Rotate`):
/// the box runs along the displayed axes, like an image signature's
/// ([`SignatureImage::rect`](crate::SignatureImage::rect)), and the outlines are turned back so the name reads across.
pub fn typed_signature_shape(at: [f64; 2], text: &str, height: f64, rotation: i64) -> Option<Shape> {
    let o = script_outline(text);
    let [left, bottom, right, top] = o.bounds();
    let span = (top - bottom).max(0.1);
    let width = right - left;
    if o.contours.is_empty() || o.width <= 0.0 || !(width > 0.0 && height > 0.0) {
        return None;
    }
    let k = height / span;
    let rect = signature_image::upright_box(rotation, at, width * k, height)?;
    // Displayed right and up as user-space unit vectors: a point at (nx, ny) of the displayed box
    // is at this fraction of the user-space box. Exact for an unturned page.
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let to_user = |nx: f64, ny: f64| [a * nx + c * ny + (-a).max(0.0) + (-c).max(0.0), b * nx + d * ny + (-b).max(0.0) + (-d).max(0.0)];
    let contours = o.contours.iter().map(|c| c.iter().map(|p| to_user((p[0] - left) / width, (p[1] - bottom) / span)).collect()).collect();
    Some(Shape::TypedSignature { rect, contours })
}

/// `at` moved `right` and `up` points as displayed on a page turned by `rotation` (its `/Rotate`).
pub fn displayed_offset(rotation: i64, at: [f64; 2], right: f64, up: f64) -> [f64; 2] {
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    [at[0] + a * right + c * up, at[1] + b * right + d * up]
}

/// The user-space box `width` x `height` points as displayed on a page turned by `rotation`,
/// with its displayed top-left corner at `at`: where typed text and text boxes go. `None` for a
/// non-finite point.
pub fn text_box_at(rotation: i64, at: [f64; 2], width: f64, height: f64) -> Option<[f64; 4]> {
    signature_image::upright_box(rotation, displayed_offset(rotation, at, 0.0, -height / 2.0), width, height)
}

/// `text` in the script font as the graphic of a visible digital signature, as Fill & Sign types
/// it. `None` for text with no outlines.
pub fn typed_signature_graphic(text: &str) -> Option<pdfcraft_sign::Graphic> {
    let o = script_outline(text);
    o.contours.iter().any(|c| c.len() > 2).then_some(pdfcraft_sign::Graphic::Outlines(o.contours))
}
