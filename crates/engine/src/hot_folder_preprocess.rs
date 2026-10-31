//! Reusable Hot Folder page selection, image preparation and recognition-area templates.
//! Image changes are opt-in and preserve physical size at the actual rendering resolution.
use crate::{Session, ocr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::sync::Arc;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// One-based page ranges, e.g. 1-3,5. Empty selects the whole document.
    pub pages: String,
    pub grayscale: bool,
    pub invert: bool,
    pub brightness: i32,
    pub contrast: f32,
    pub threshold: Option<u8>,
    pub median_filter: bool,
    /// Split a spread vertically into a left and right page, in reading order.
    pub split_spreads: bool,
    pub regions: Vec<Region>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Region {
    /// Zero applies to every prepared page; otherwise a one-based prepared page number.
    pub page: usize,
    /// Fractions of displayed page size, from top-left: x, y, width, height.
    pub rect: [f32; 4],
    /// Included areas constrain recognition; excluded areas suppress words inside them.
    pub exclude: bool,
}
impl Default for Region {
    fn default() -> Self {
        Self { page: 0, rect: [0.0, 0.0, 1.0, 1.0], exclude: false }
    }
}

impl Settings {
    pub fn changes_images(&self) -> bool {
        self.grayscale
            || self.invert
            || self.brightness != 0
            || self.contrast != 0.0
            || self.threshold.is_some()
            || self.median_filter
            || self.split_spreads
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.pages.len() > 4096
            || self.regions.len() > 64
            || !(-100..=100).contains(&self.brightness)
            || !self.contrast.is_finite()
            || !(-100.0..=100.0).contains(&self.contrast)
        {
            return Err("image settings exceed allowed ranges (brightness/contrast -100..100, at most 64 regions)".into());
        }
        parse_pages(&self.pages, None)?;
        for region in &self.regions {
            let [x, y, w, h] = region.rect;
            if !region.rect.iter().all(|v| v.is_finite())
                || x < 0.0
                || y < 0.0
                || w <= 0.0
                || h <= 0.0
                || x + w > 1.00001
                || y + h > 1.00001
                || region.page > 10_000
            {
                return Err("recognition regions must fit within the page (coordinates 0..1)".into());
            }
        }
        Ok(())
    }
}

pub fn parse_pages(value: &str, count: Option<usize>) -> Result<Vec<usize>, String> {
    if value.trim().is_empty() {
        return Ok(count.map_or_else(Vec::new, |n| (0..n).collect()));
    }
    let mut pages = BTreeSet::new();
    for range in value.split(',') {
        let mut parts = range.trim().split('-');
        let first =
            parts.next().unwrap_or_default().trim().parse::<usize>().map_err(|_| "pages must be one-based numbers/ranges, for example 1-3,5")?;
        let last = parts.next().map(|v| v.trim().parse::<usize>()).transpose().map_err(|_| "invalid page range")?.unwrap_or(first);
        if parts.next().is_some() || first == 0 || last < first || last > count.unwrap_or(10_000) {
            return Err("page range is reversed or outside the document".into());
        }
        for page in first..=last {
            pages.insert(page - 1);
        }
    }
    Ok(pages.into_iter().collect())
}

pub fn prepare(bytes: Arc<Vec<u8>>, settings: &Settings, dpi: f64, should_stop: &dyn Fn() -> bool) -> Result<Arc<Vec<u8>>, String> {
    prepare_bounded(bytes, settings, dpi, 512 * 1024 * 1024, Session::new().now_secs(), should_stop)
}

pub(crate) fn prepare_bounded(
    bytes: Arc<Vec<u8>>,
    settings: &Settings,
    dpi: f64,
    budget: u64,
    timestamp: i64,
    should_stop: &dyn Fn() -> bool,
) -> Result<Arc<Vec<u8>>, String> {
    settings.validate()?;
    if bytes.len() as u64 > budget {
        return Err("prepared input exceeds remaining batch memory budget; use smaller batches".into());
    }
    if !settings.changes_images() && settings.pages.trim().is_empty() {
        return Ok(bytes);
    }
    let mut session = Session::new().with_timestamp(timestamp);
    let id = session.open("input.pdf", None, bytes, None).map_err(|e| e.to_string())?;
    let doc = session.get(id).ok_or("document unavailable")?;
    if let Some(reason) = &doc.read_only_reason {
        return Err(reason.clone());
    }
    let selected = parse_pages(&settings.pages, Some(doc.info.pages.len()))?;
    if !settings.changes_images() {
        return session.extract(id, &selected).map_err(|e| e.to_string());
    }
    // A raster replacement cannot preserve interactive structures. Refuse to silently flatten them.
    if !doc.info.annotations.is_empty() || !doc.info.fields.is_empty() || !doc.info.links.is_empty() {
        return Err("image preprocessing cannot preserve annotations, links or forms; disable image changes for this document".into());
    }
    let mut exporter = crate::export::Exporter::new(doc);
    let mut sources = Vec::new();
    let mut total = 0_u64;
    for page in selected {
        if should_stop() {
            return Err("task stopped during image preprocessing".into());
        }
        let png = exporter.png(page, dpi)?;
        let mut rgb = image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgb8();
        transform(&mut rgb, settings);
        let slices = if settings.split_spreads {
            vec![(0, rgb.width() / 2), (rgb.width() / 2, rgb.width() - rgb.width() / 2)]
        } else {
            vec![(0, rgb.width())]
        };
        for (left, width) in slices {
            if width == 0 {
                return Err("page is too narrow to split".into());
            }
            let image = image::imageops::crop_imm(&rgb, left, 0, width, rgb.height()).to_image();
            let mut encoded = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image).write_to(&mut encoded, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            if total.saturating_add(encoded.get_ref().len() as u64) > budget {
                return Err("prepared images exceed 512 MiB; reduce resolution or select fewer pages".into());
            }
            let name = format!("page-{}-{left}.png", page.saturating_add(1));
            let pdf = session
                .create_from_images_with_resolution(
                    &[(name.clone(), encoded.into_inner())],
                    crate::ImageResolution::Dpi(exporter.dpi_used(page, dpi)),
                )
                .map_err(|e| e.to_string())?;
            total = total.saturating_add(pdf.len() as u64);
            if total > budget {
                return Err("prepared PDFs exceed remaining batch memory budget; use smaller batches".into());
            }
            sources.push((name, pdf));
        }
    }
    let combined = session.combine(&sources).map_err(|e| e.to_string())?;
    if combined.len() as u64 > budget {
        return Err("combined prepared PDF exceeds remaining batch memory budget".into());
    }
    Ok(combined)
}

fn transform(image: &mut image::RgbImage, settings: &Settings) {
    if settings.median_filter {
        let source = image.clone();
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            for (channel, value) in pixel.0.iter_mut().enumerate() {
                let mut values = [255u8; 9];
                let mut count = 0usize;
                for row in y.saturating_sub(1)..=y.saturating_add(1).min(source.height().saturating_sub(1)) {
                    for col in x.saturating_sub(1)..=x.saturating_add(1).min(source.width().saturating_sub(1)) {
                        if let Some(value) = values.get_mut(count) {
                            *value = source.get_pixel(col, row).0.get(channel).copied().unwrap_or(255);
                            count += 1;
                        }
                    }
                }
                if let Some(values) = values.get_mut(..count) {
                    values.sort_unstable();
                    *value = values.get(count / 2).copied().unwrap_or(*value);
                }
            }
        }
    }
    let factor = ((100.0 + settings.contrast) / 100.0).powi(2);
    for pixel in image.pixels_mut() {
        for value in &mut pixel.0 {
            let level = (f32::from(*value) - 127.5) * factor + 127.5 + settings.brightness as f32 * 2.55;
            *value = level.round().clamp(0.0, 255.0) as u8;
            if settings.invert {
                *value = 255 - *value;
            }
        }
        if settings.grayscale || settings.threshold.is_some() {
            let [r, g, b] = pixel.0;
            let mut level = ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8;
            if let Some(threshold) = settings.threshold {
                level = if level < threshold { 0 } else { 255 };
            }
            pixel.0 = [level; 3];
        }
    }
}

/// Regions constrain only new OCR text; visible page content is preserved.
pub fn constrain_words(pages: &mut [ocr::OcrPage], infos: &[pdfcraft_render::PageInfo], regions: &[Region]) {
    for page in pages {
        let Some(info) = infos.get(page.page) else { continue };
        let applies = |r: &&Region| r.page == 0 || r.page == page.page.saturating_add(1);
        let selected: Vec<_> = regions.iter().filter(applies).collect();
        page.words.retain(|word| {
            let x = word.origin[0] + (word.across[0] + word.up[0]) * 0.5;
            let y = word.origin[1] + (word.across[1] + word.up[1]) * 0.5;
            let [x, y] = info.user_to_view(x as f32, y as f32);
            let x = x / info.width.max(0.001);
            let y = y / info.height.max(0.001);
            let contains = |r: &&Region| {
                let [left, top, w, h] = r.rect;
                x >= left && x <= left + w && y >= top && y <= top + h
            };
            (!selected.iter().any(|r| !r.exclude) || selected.iter().filter(|r| !r.exclude).any(contains))
                && !selected.iter().filter(|r| r.exclude).any(contains)
        });
    }
}

pub struct BlankResult {
    pub document: Option<Arc<Vec<u8>>>,
    pub blank_pages: Option<Arc<Vec<u8>>>,
}
/// Strict white-only detection retains faint marks, existing text and interactive content.
pub fn separate_blank_pages(bytes: Arc<Vec<u8>>, should_stop: &dyn Fn() -> bool) -> Result<BlankResult, String> {
    separate_blank_pages_at(bytes, Session::new().now_secs(), should_stop)
}

pub(crate) fn separate_blank_pages_at(bytes: Arc<Vec<u8>>, timestamp: i64, should_stop: &dyn Fn() -> bool) -> Result<BlankResult, String> {
    let mut session = Session::new().with_timestamp(timestamp);
    let id = session.open("scan.pdf", None, bytes.clone(), None).map_err(|e| e.to_string())?;
    let doc = session.get(id).ok_or("document unavailable")?;
    if !doc.info.annotations.is_empty() || !doc.info.fields.is_empty() || !doc.info.links.is_empty() {
        return Ok(BlankResult { document: Some(bytes), blank_pages: None });
    }
    let count = doc.info.pages.len();
    let mut exporter = crate::export::Exporter::new(doc);
    let mut blank = Vec::new();
    for page in 0..count {
        if should_stop() {
            return Err("task stopped while detecting blank pages".into());
        }
        if !exporter.text(page)?.trim().is_empty() {
            continue;
        }
        let png = exporter.png(page, 150.0)?;
        let image = image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgb8();
        if image.pixels().all(|p| p.0 == [255; 3]) {
            blank.push(page);
        }
    }
    if blank.is_empty() {
        return Ok(BlankResult { document: Some(bytes), blank_pages: None });
    }
    let archived = session.extract(id, &blank).map_err(|e| e.to_string())?;
    let keep: Vec<_> = (0..count).filter(|p| !blank.contains(p)).collect();
    let document = if keep.is_empty() { None } else { Some(session.extract(id, &keep).map_err(|e| e.to_string())?) };
    Ok(BlankResult { document, blank_pages: Some(archived) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hot_folder_page_ranges_are_bounded_and_sorted() {
        assert_eq!(parse_pages("3,1-2,2", Some(3)).unwrap(), vec![0, 1, 2]);
        for value in ["0", "3-1", "1-10001", "NaN", "1-2-3"] {
            assert!(parse_pages(value, Some(10)).is_err());
        }
    }
    #[test]
    fn hot_folder_image_options_change_pixels_without_touching_the_source() {
        let mut image = image::RgbImage::from_pixel(3, 3, image::Rgb([0, 0, 0]));
        image.put_pixel(1, 1, image::Rgb([255, 255, 255]));
        transform(&mut image, &Settings { median_filter: true, invert: true, ..Default::default() });
        assert!(image.pixels().all(|p| p.0 == [255; 3]));
        let mut image = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 0, 0]));
        transform(&mut image, &Settings { threshold: Some(128), ..Default::default() });
        assert!(image.pixels().all(|p| p.0 == [0; 3]));
    }
    #[test]
    fn hot_folder_blank_pages_can_be_archived_even_when_all_are_blank() {
        let bytes = Session::new().create_blank(100.0, 100.0, 2).unwrap();
        let result = separate_blank_pages(bytes, &|| false).unwrap();
        assert!(result.document.is_none());
        let mut s = Session::new();
        let id = s.open("blank", None, result.blank_pages.unwrap(), None).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
    }
    #[test]
    fn hot_folder_spread_split_preserves_physical_size() {
        let bytes = Session::new().create_blank(200.0, 100.0, 1).unwrap();
        let result = prepare(bytes, &Settings { split_spreads: true, ..Default::default() }, 72.0, &|| false).unwrap();
        let mut s = Session::new();
        let id = s.open("split", None, result, None).unwrap();
        let pages = &s.get(id).unwrap().info.pages;
        assert_eq!(pages.len(), 2);
        for page in pages {
            assert!((page.width - 100.0).abs() < 0.1 && (page.height - 100.0).abs() < 0.1);
        }
    }
}
