//! Fill & Sign geometry: typed signatures, and boxes placed as a turned page displays them.

use crate::{Shape, SyriacFamily, script_outline, script_outline_for_family, signature_image};

/// Fill & Sign: `text` in the script font as a typed signature, its left edge at `at` (user
/// space, vertically centred) and `height` points tall. `None` for text with no outlines.
///
/// Left, centred and upright are as displayed on a page turned by `rotation` (its `/Rotate`):
/// the box runs along the displayed axes, like an image signature's
/// ([`SignatureImage::rect`](crate::SignatureImage::rect)), and the outlines are turned back so the name reads across.
pub fn typed_signature_shape(at: [f64; 2], text: &str, height: f64, rotation: i64) -> Option<Shape> {
    typed_signature_shape_with_family(at, text, height, rotation, SyriacFamily::default())
}

/// Like [`typed_signature_shape`], using the selected family for Syriac signature outlines.
pub fn typed_signature_shape_with_family(at: [f64; 2], text: &str, height: f64, rotation: i64, family: SyriacFamily) -> Option<Shape> {
    let o = script_outline_for_family(text, family);
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

#[cfg(test)]
mod arabic_signature_tests {
    /// Arabic-only ink survives outline normalization, mixed names stay upright through
    /// all page rotations, and one unsupported character refuses the complete name.
    #[test]
    fn arabic_typed_signature_keeps_rotated_geometry_and_rejects_partial_names() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            assert!(std::env::var_os("CRAFT_FONTS_REQUIRED").is_none(), "Arabic font required");
            return;
        }
        // The original mixed-name test is a supplementary geometry control: the old
        // Latin-only partial output could satisfy it. Arabic-only ink must exist too.
        let arabic = "سَلام";
        let outline = crate::script_outline(arabic);
        assert!(!outline.contours.is_empty());
        let graphic = super::typed_signature_graphic(arabic).expect("Arabic-only graphic");
        let pdfcraft_sign::Graphic::Outlines(graphic_contours) = graphic else { panic!("wrong graphic") };
        assert_eq!(graphic_contours, outline.contours);
        let shape = super::typed_signature_shape([200.0, 200.0], arabic, 24.0, 0).expect("Arabic-only shape");
        let crate::Shape::TypedSignature { contours, .. } = shape else { panic!("wrong shape") };
        let [left, bottom, right, top] = outline.bounds();
        let expected: Vec<Vec<[f64; 2]>> = outline
            .contours
            .iter()
            .map(|contour| contour.iter().map(|p| [(p[0] - left) / (right - left), (p[1] - bottom) / (top - bottom).max(0.1)]).collect())
            .collect();
        assert_eq!(contours, expected, "shape normalization must preserve all Arabic ink");
        let name = "سَلام 123 (Ada)";
        assert!(super::typed_signature_graphic(name).is_some());
        let mut dimensions = Vec::new();
        for rotation in [0, 90, 180, 270] {
            let shape = super::typed_signature_shape([200.0, 200.0], name, 24.0, rotation).expect("Arabic typed signature");
            let crate::Shape::TypedSignature { rect, contours } = shape else { panic!("wrong shape") };
            assert!(rect.iter().all(|v| v.is_finite()));
            assert!(!contours.is_empty());
            assert!(contours.iter().flatten().all(|p| p.iter().all(|v| v.is_finite() && (-0.000001..=1.000001).contains(v))));
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
            dimensions.push(if rotation % 180 == 0 { (w, h) } else { (h, w) });
        }
        for (w, h) in dimensions {
            assert!(w > 24.0 && (h - 24.0).abs() < 0.000001);
        }
        assert!(super::typed_signature_shape([0.0, 0.0], "سارة 张", 24.0, 0).is_none());
        assert!(super::typed_signature_graphic("سارة 张").is_none());
    }
}
