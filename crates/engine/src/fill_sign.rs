//! Fill & Sign geometry: typed signatures, and boxes placed as a turned page displays them.

use pdfcraft_render::PageInfo;

use crate::{Document, Edit, EditError, Label, MarkFile, NewAnnotation, Shape, guard, script_outline, signature_image};

/// An explicit appearance-signature size in physical page points, independent of zoom
/// and the target page's rotation or UserUnit. Local preferences may store its two getters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignaturePlacementSize {
    width: f64,
    height: f64,
}

impl SignaturePlacementSize {
    /// Each side must be between 0.01 and 14400 physical points. These preference limits
    /// keep even very thin image signatures usable without accepting unbounded geometry.
    pub fn new(width: f64, height: f64) -> Result<Self, EditError> {
        if ![width, height].iter().all(|v| v.is_finite() && (0.01..=14400.0).contains(v)) {
            return Err(EditError::Invalid("signature width and height must each be 0.01–14400 physical points".into()));
        }
        Ok(Self { width, height })
    }

    pub fn width(self) -> f64 {
        self.width
    }

    pub fn height(self) -> f64 {
        self.height
    }

    /// Check whether the saved drawing can fit this physical size without thinning its
    /// default stroke or clipping its curves. This does not depend on the eventual page.
    pub fn validate_drawing<T: Copy + Into<f64>>(self, strokes: &[Vec<[T; 2]>]) -> Result<(), EditError> {
        let width = crate::Style::default_for(&Shape::Signature { strokes: Vec::new() }).width;
        pdfcraft_annot::validate_signature_size(strokes, width, [self.width, self.height])?;
        Ok(())
    }

    /// The user-space box, left-aligned and vertically centered at `at`, upright as the
    /// page is displayed. The same conversion is used for placement and cursor previews.
    pub fn rect_at(self, page: &PageInfo, at: [f64; 2]) -> Result<[f64; 4], EditError> {
        let [sx, sy] = page_units(page)?;
        let (width_unit, height_unit) = if page.rotation.is_multiple_of(180) { (sx, sy) } else { (sy, sx) };
        let rect = signature_image::upright_box(i64::from(page.rotation), at, self.width / width_unit, self.height / height_unit)
            .ok_or_else(|| EditError::Invalid("signature position must be finite".into()))?;
        if !rect.iter().all(|v| v.is_finite() && v.abs() < 1e7) || rect[2] <= rect[0] || rect[3] <= rect[1] {
            return Err(EditError::Invalid("signature size or position cannot be represented on this page".into()));
        }
        Ok(rect)
    }

    fn from_rect(page: &PageInfo, rect: [f64; 4]) -> Result<Self, EditError> {
        let [sx, sy] = page_units(page)?;
        let (w, h) = ((rect[2] - rect[0]) * sx, (rect[3] - rect[1]) * sy);
        if page.rotation.is_multiple_of(180) { Self::new(w, h) } else { Self::new(h, w) }
    }
}

/// Physical points per user-space unit on each unrotated axis. Invalid transforms are
/// refused rather than using PageInfo's interactive denominator clamps for a preference.
fn page_units(page: &PageInfo) -> Result<[f64; 2], EditError> {
    let [x0, y0, x1, y1] = page.crop.map(f64::from);
    let (w, h) = (f64::from(page.width), f64::from(page.height));
    if !matches!(page.rotation, 0 | 90 | 180 | 270)
        || ![x0, y0, x1, y1, w, h].iter().all(|v| v.is_finite())
        || x1 <= x0
        || y1 <= y0
        || w <= 0.0
        || h <= 0.0
    {
        return Err(EditError::Invalid("the page has invalid signature placement geometry".into()));
    }
    let (pw, ph) = if page.rotation.is_multiple_of(180) { (w, h) } else { (h, w) };
    let (sx, sy) = (pw / (x1 - x0), ph / (y1 - y0));
    if ![sx, sy].iter().all(|v| v.is_finite() && *v > 0.0) || (sx - sy).abs() > sx.max(sy) * 1e-4 {
        return Err(EditError::Invalid("the page has invalid signature placement geometry".into()));
    }
    Ok([sx, sy])
}

/// A validated add-and-size operation. Its private source cannot contain arbitrary nested
/// edits, annotation indices or local preference identities.
#[derive(Clone, Debug, PartialEq)]
pub struct SizedSignaturePlacement {
    pub(crate) source: SignaturePlacementSource,
    pub(crate) rect: [f64; 4],
    pub(crate) label: Label,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SignaturePlacementSource {
    Annotation(NewAnnotation),
    Image { page: usize, rect: [f64; 4], name: String, file: MarkFile, author: String },
}

/// Honor an explicit size on an existing Fill & Sign placement edit. Callers without a
/// preference keep their original edit, including its legacy sizing and image fit policy.
/// Only a drawn/typed signature or one image-signature placement is accepted.
pub fn signature_placement_with_size(edit: Edit, page: &PageInfo, at: [f64; 2], size: SignaturePlacementSize) -> Result<Edit, EditError> {
    let rect = size.rect_at(page, at)?;
    let label = edit.label();
    let edit = match edit {
        Edit::Batch { mut edits, .. } if edits.len() == 1 => edits.pop(),
        edit => Some(edit),
    };
    let source = match edit {
        Some(Edit::AddAnnotation(a)) if matches!(a.shape, Shape::Signature { .. } | Shape::TypedSignature { .. }) => {
            SignaturePlacementSource::Annotation(a)
        }
        Some(Edit::AddCustomStamp { page, rect, name, file, author }) if matches!(name.as_str(), "Signature" | "Initials") => {
            SignaturePlacementSource::Image { page, rect, name, file, author }
        }
        _ => return Err(EditError::Invalid("an explicit signature size needs one typed, drawn or image signature placement".into())),
    };
    Ok(Edit::AddSizedSignature(SizedSignaturePlacement { source, rect, label }))
}

impl Document {
    /// Read the physical size of a suitable selected appearance. This is an explicit
    /// geometry-copy source, not proof that a particular local saved graphic created it.
    pub fn signature_placement_size(&self, page: usize, index: usize) -> Result<Option<SignaturePlacementSize>, EditError> {
        let info = self.info.pages.get(page).ok_or_else(|| EditError::Invalid("the signature page does not exist".into()))?;
        let editor = self.editor.as_ref().ok_or_else(|| EditError::ReadOnly("the document cannot be read".into()))?;
        guard(|| {
            pdfcraft_annot::appearance_signature_rect(&editor.cos, page, index)?.map(|rect| SignaturePlacementSize::from_rect(info, rect)).transpose()
        })
        .map_err(EditError::Invalid)?
    }
}

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
