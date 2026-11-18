//! An imposed sheet rendered for explicit native file printing. Never uses installed fonts.

use std::sync::Arc;

use pdfcraft_render::{MAX_PIXELS, PRINT_TAG, PageRenderer, RenderConfig, RenderRequest, device_pixels, effective_scale};

/// One opaque RGB sheet, with its physical size in points.
pub struct SheetRgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub paper: (f64, f64),
}

/// Render the already imposed PDF once, at exactly the requested resolution. Refuse a
/// reduced or partial raster instead of silently losing print detail.
pub fn render_sheet(bytes: Vec<u8>, dpi: f64, grayscale: bool) -> Result<SheetRgb, String> {
    crate::guard(|| {
        if !(72.0..=600.0).contains(&dpi) {
            return Err("native output dpi must be between 72 and 600".into());
        }
        let bytes = Arc::new(bytes);
        let info = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| e.to_string())?;
        if info.pages.len() != 1 {
            return Err("native file output supports exactly one imposed sheet".into());
        }
        let page = info.pages.first().ok_or("native output has no sheet")?;
        let (w, h) = (page.width, page.height);
        let scale = (dpi / 72.0) as f32;
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) || effective_scale(w, h, scale) < scale {
            return Err("native output exceeds renderer limits at the requested dpi; choose a lower dpi or smaller paper".into());
        }
        let expected = (device_pixels(w, scale), device_pixels(h, scale));
        let mut renderer = PageRenderer::new(bytes, RenderConfig { reject_oversize: true, system_fonts: false, ..Default::default() });
        let rendered = renderer.render(RenderRequest { page: 0, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(error) = rendered.error {
            return Err(error);
        }
        if !rendered.warnings.is_empty() {
            return Err(format!("native output refused a partial raster: {:?}", rendered.warnings));
        }
        if (rendered.width, rendered.height) != expected {
            return Err("native output did not retain the requested raster resolution".into());
        }
        let pixels = u64::from(rendered.width)
            .checked_mul(u64::from(rendered.height))
            .filter(|p| *p <= MAX_PIXELS as u64)
            .ok_or("native output exceeds the raster pixel limit")?;
        let length = pixels.checked_mul(3).and_then(|n| usize::try_from(n).ok()).ok_or("native output image is too large")?;
        if pixels.checked_mul(4) != Some(rendered.rgba.len() as u64) {
            return Err("native output has incomplete raster pixels".into());
        }
        let mut data = Vec::new();
        data.try_reserve_exact(length).map_err(|_| "not enough memory for native output pixels")?;
        for px in rendered.rgba.as_chunks::<4>().0 {
            let gap = 255 - u32::from(px[3]);
            let (r, g, b) = ((u32::from(px[0]) + gap).min(255), (u32::from(px[1]) + gap).min(255), (u32::from(px[2]) + gap).min(255));
            let rgb = if grayscale {
                let y = ((299 * r + 587 * g + 114 * b) / 1000) as u8;
                [y, y, y]
            } else {
                [r as u8, g as u8, b as u8]
            };
            data.extend_from_slice(&rgb);
        }
        Ok(SheetRgb { width: rendered.width, height: rendered.height, data, paper: (f64::from(w), f64::from(h)) })
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: f64, h: f64, pages: usize) -> Vec<u8> {
        let doc = pdfcraft_create::blank(w, h, pages).unwrap();
        pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
    }

    #[test]
    fn native_sheet_retains_paper_and_requested_resolution() {
        let image = render_sheet(blank(72.0, 144.0, 1), 144.0, true).unwrap();
        assert_eq!((image.width, image.height, image.paper), (144, 288, (72.0, 144.0)));
        assert_eq!(image.data.len(), 144 * 288 * 3);
        assert!(image.data.iter().all(|v| *v == 255));
    }

    #[test]
    fn native_sheet_applies_grayscale_only_when_requested() {
        let doc = pdfcraft_create::from_contents(&[(72.0, 72.0, pdfcraft_cos::Dict::new(), b"1 0 0 rg 0 0 72 72 re f".to_vec())]).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let color = render_sheet(bytes.clone(), 72.0, false).unwrap();
        let gray = render_sheet(bytes, 72.0, true).unwrap();
        assert_eq!(color.data.get(300..303), Some([255, 0, 0].as_slice()));
        assert_eq!(gray.data.get(300..303), Some([76, 76, 76].as_slice()));
    }

    #[test]
    fn native_sheet_refuses_multiple_sheets_and_silent_dpi_reduction() {
        assert!(render_sheet(blank(72.0, 72.0, 2), 72.0, false).is_err());
        let error = render_sheet(blank(14_400.0, 14_400.0, 1), 600.0, false).err().unwrap();
        assert!(error.contains("requested dpi"), "{error}");
        for dpi in [f64::NAN, f64::INFINITY, 0.0, 601.0] {
            assert!(render_sheet(blank(72.0, 72.0, 1), dpi, false).is_err());
        }
    }
}
