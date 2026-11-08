//! Compare files: the text differences between two open documents, a report, and the
//! differences marked as comments in the newer one.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::ObjRef;

pub use pdfcraft_compare::{Change, Comparison, Kind, Side};
// How far Office export reads into form XObjects; `OfficeExport::truncated_pages` lists the pages
// that draw more.
pub use pdfcraft_edit::{MAX_FORM_DEPTH, MAX_FORM_VISITS};

use crate::{DocId, Edit, EditError, Markup, NewAnnotation, NoteIcon, Session, Shape};

/// Most bytes of encoded pictures one Office export prepares, counting each image object once
/// (the Word and PowerPoint writers keep at most 512 MiB of them as well).
pub const MAX_EXPORT_IMAGE_BYTES: usize = 512 << 20;

/// What a visual difference is, see [`Session::visual_kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualKind {
    Annotation,
    Image,
    Other,
}

impl VisualKind {
    pub fn label(self) -> &'static str {
        match self {
            VisualKind::Annotation => "annotation",
            VisualKind::Image => "image",
            VisualKind::Other => "other",
        }
    }
}

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

impl crate::Document {
    /// Standards ▸ Verify PDF/A: the rules the document breaks for `level`.
    pub fn pdfa_verify(&self, level: pdfcraft_preflight::Level) -> Vec<pdfcraft_preflight::Issue> {
        self.editor.as_ref().map(|e| pdfcraft_preflight::verify(&e.cos, level)).unwrap_or_default()
    }

    /// The standards the document declares (Standards panel).
    pub fn standards(&self) -> pdfcraft_preflight::Declared {
        self.editor.as_ref().map(|e| pdfcraft_preflight::declared(&e.cos)).unwrap_or_default()
    }

    /// Every word of the document in reading order, with page and box.
    pub fn words(&self) -> Vec<pdfcraft_compare::Word> {
        let config = pdfcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(t) = res.text {
                out.extend(crate::js::page_words(&t, info).into_iter().map(|(text, rect)| pdfcraft_compare::Word { text, page, rect }));
            }
        }
        out
    }
}

impl Session {
    /// Compare files: the text differences from document `old` to document `new`. The result
    /// carries both documents' text, so both must allow copying it.
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        a.check_copying()?;
        b.check_copying()?;
        Ok(pdfcraft_compare::compare(&a.words(), &b.words()))
    }

    /// [`Session::compare`], keeping the changes on `pages` of `new` (0-based; empty: every page).
    pub fn compare_pages(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Comparison, EditError> {
        let mut c = self.compare(old, new)?;
        if !pages.is_empty() {
            c.changes.retain(|ch| pages.iter().any(|page| (ch.new.page..=ch.new.last_page).contains(page)));
        }
        Ok(c)
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`).
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        let renderer = |d: &crate::Document| {
            pdfcraft_render::PageRenderer::new(
                d.bytes.clone(),
                pdfcraft_render::RenderConfig { password: d.password.as_deref().map(Arc::from), ..Default::default() },
            )
        };
        let (mut ra, mut rb) = (renderer(a), renderer(b));
        let scale = dpi.clamp(18.0, 150.0) / 72.0;
        let mut out = Vec::new();
        for (page, info) in b.info.pages.iter().enumerate().take(a.info.pages.len()) {
            let req = pdfcraft_render::RenderRequest { page, scale, ..Default::default() };
            let (x, y) = (ra.render(req), rb.render(req));
            if x.error.is_some() || y.error.is_some() {
                continue;
            }
            let s = y.width as f32 / info.width.max(1e-3);
            for r in pdfcraft_compare::visual_regions((&y.rgba, y.width, y.height), (&x.rgba, x.width, x.height), 24) {
                let p = info.view_to_user(r[0] as f32 / s, r[1] as f32 / s);
                let q = info.view_to_user(r[2] as f32 / s, r[3] as f32 / s);
                out.push((page, [p[0].min(q[0]) as f64, p[1].min(q[1]) as f64, p[0].max(q[0]) as f64, p[1].max(q[1]) as f64]));
            }
        }
        Ok(out)
    }

    /// What a visual difference at `rect` on `page` (user space, as `compare_visual` returns it)
    /// is: a comment or form mark drawn there in either file, else a picture drawn there in
    /// either file, else some other drawing (Acrobat's finer kinds of change).
    pub fn visual_kind(&self, old: DocId, new: DocId, page: usize, rect: [f64; 4]) -> Result<VisualKind, EditError> {
        let docs = [self.get(old).ok_or(EditError::NoDocument)?, self.get(new).ok_or(EditError::NoDocument)?];
        let images = [raster_image_rects(docs[0], page)?, raster_image_rects(docs[1], page)?];
        let old_rect = map_rect_to_page(rect, docs[1].info.pages.get(page), docs[0].info.pages.get(page)).unwrap_or(rect);
        Ok(classify_visual_kind(docs, page, rect, old_rect, [&images[0], &images[1]]))
    }

    /// Classify many visual regions while reading each page's drawn-image inventory once.
    pub fn visual_kinds(&self, old: DocId, new: DocId, regions: &[(usize, [f64; 4])]) -> Result<Vec<VisualKind>, EditError> {
        let docs = [self.get(old).ok_or(EditError::NoDocument)?, self.get(new).ok_or(EditError::NoDocument)?];
        let mut image_rects: HashMap<usize, [Vec<[f64; 4]>; 2]> = HashMap::new();
        for (page, _) in regions {
            if !image_rects.contains_key(page) {
                image_rects.insert(*page, [raster_image_rects(docs[0], *page)?, raster_image_rects(docs[1], *page)?]);
            }
        }
        regions
            .iter()
            .map(|(page, rect)| {
                let old_rect = map_rect_to_page(*rect, docs[1].info.pages.get(*page), docs[0].info.pages.get(*page)).unwrap_or(*rect);
                let empty: [Vec<[f64; 4]>; 2] = [Vec::new(), Vec::new()];
                let images = image_rects.get(page).unwrap_or(&empty);
                Ok(classify_visual_kind(docs, *page, *rect, old_rect, [&images[0], &images[1]]))
            })
            .collect()
    }

    /// The compare report as a new PDF (not opened).
    pub fn compare_report(&self, old: DocId, new: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        self.compare_report_of(old, new, &[])
    }

    /// [`Session::compare_report`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn compare_report_of(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Arc<Vec<u8>>, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let name = |id| self.get(id).map(|d| d.name.clone()).unwrap_or_default();
        self.create_report("Compare Report", &pdfcraft_compare::report(&c, &name(old), &name(new)))
    }

    /// Mark the differences in `new` as comments: highlights over replaced and inserted text
    /// (blue, green) and a note where text was deleted (red), authored "Compare". One undoable
    /// step; returns how many comments were added.
    pub fn mark_differences(&mut self, old: DocId, new: DocId) -> Result<usize, EditError> {
        self.mark_differences_of(old, new, &[])
    }

    /// [`Session::mark_differences`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn mark_differences_of(&mut self, old: DocId, new: DocId, pages: &[usize]) -> Result<usize, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let mut edits = Vec::new();
        for ch in &c.changes {
            let color = colour(ch.kind);
            let contents = match ch.kind {
                Kind::Replaced => format!("Replaced: \"{}\" with \"{}\"", ch.old.text, ch.new.text),
                Kind::Inserted => format!("Inserted: \"{}\"", ch.new.text),
                Kind::Deleted => format!("Deleted: \"{}\"", ch.old.text),
            };
            let shape = if ch.new.rects.is_empty() {
                let Some(r) = ch.new.near else { continue };
                Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note }
            } else {
                Shape::TextMarkup {
                    kind: Markup::Highlight,
                    quads: ch.new.rects.iter().map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]).collect(),
                }
            };
            let mut style = crate::Style::default_for(&shape);
            style.color = color;
            edits.push(Edit::AddAnnotation(NewAnnotation { page: ch.new.page, shape, style, contents, author: "Compare".into() }));
        }
        let n = edits.len();
        if n > 0 {
            self.apply(new, Edit::Batch { label: "Mark differences".into(), edits })?;
        }
        Ok(n)
    }
}

fn raster_image_rects(doc: &crate::Document, page: usize) -> Result<Vec<[f64; 4]>, EditError> {
    let Some(editor) = doc.editor.as_ref() else { return Ok(Vec::new()) };
    let mut rects: Vec<[f64; 4]> = pdfcraft_edit::reading_images(&editor.cos, page)?.into_iter().filter_map(|image| image.visible_rect).collect();
    rects.extend(pdfcraft_edit::reading_inline_image_rects(&editor.cos, page)?);
    Ok(rects)
}

fn classify_visual_kind(docs: [&crate::Document; 2], page: usize, new_rect: [f64; 4], old_rect: [f64; 4], images: [&[[f64; 4]]; 2]) -> VisualKind {
    let hits = |a: [f64; 4], b: [f64; 4]| a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
    let annotation_hits = |doc: &crate::Document, rect: [f64; 4]| {
        let rotation = doc.info.pages.get(page).map_or(0, |p| p.rotation);
        let comments =
            doc.info.annotations.iter().any(|a| a.page == page && a.subtype != "Link" && hits(a.shown_rect(rotation).map(f64::from), rect));
        let widgets = doc.form.iter().flat_map(|field| &field.widgets).any(|w| w.page == Some(page) && !w.hidden && hits(w.rect, rect));
        comments || widgets
    };
    if annotation_hits(docs[0], old_rect) || annotation_hits(docs[1], new_rect) {
        return VisualKind::Annotation;
    }
    let pictured = images[0].iter().any(|image| hits(*image, old_rect)) || images[1].iter().any(|image| hits(*image, new_rect));
    if pictured { VisualKind::Image } else { VisualKind::Other }
}

/// Map a rectangle through the relative displayed-page coordinates used by `visual_regions`.
fn map_rect_to_page(rect: [f64; 4], from: Option<&pdfcraft_render::PageInfo>, to: Option<&pdfcraft_render::PageInfo>) -> Option<[f64; 4]> {
    let (from, to) = (from?, to?);
    let [x0, y0, x1, y1] = rect;
    let corners = [[x0, y0], [x0, y1], [x1, y0], [x1, y1]];
    let points = corners.map(|[x, y]| {
        let view = from.user_to_view(x as f32, y as f32);
        to.view_to_user(view[0] * to.width / from.width.max(1e-3), view[1] * to.height / from.height.max(1e-3))
    });
    Some([
        points.iter().map(|p| p[0] as f64).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p[1] as f64).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p[0] as f64).fold(f64::NEG_INFINITY, f64::max),
        points.iter().map(|p| p[1] as f64).fold(f64::NEG_INFINITY, f64::max),
    ])
}

#[cfg(test)]
mod visual_kind_tests {
    use super::*;

    #[test]
    fn maps_regions_between_cropped_and_rotated_pages() {
        let new = pdfcraft_render::PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [0.0, 0.0, 200.0, 300.0], rotation: 90 };
        let old = pdfcraft_render::PageInfo { width: 200.0, height: 300.0, label: "1".into(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 0 };
        let mapped = map_rect_to_page([40.0, 220.0, 60.0, 230.0], Some(&new), Some(&old)).expect("both pages exist");
        assert!((mapped[0] - 156.67).abs() < 0.02 && (mapped[1] - 230.0).abs() < 0.02, "{mapped:?}");
        assert!((mapped[2] - 163.33).abs() < 0.02 && (mapped[3] - 260.0).abs() < 0.02, "{mapped:?}");
    }
}

/// Export a PDF ▸ Word, PowerPoint, HTML or RTF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    Docx,
    Pptx,
    Html,
    /// CSS-free HTML that preserves text and inline images but drops colours and layout styling.
    HtmlSimple,
    Rtf,
    Markdown,
}

impl OfficeFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Pptx => "pptx",
            OfficeFormat::Html => "html",
            OfficeFormat::HtmlSimple => "html",
            OfficeFormat::Rtf => "rtf",
            OfficeFormat::Markdown => "md",
        }
    }

    pub fn from_extension(ext: &str) -> Option<OfficeFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some(OfficeFormat::Docx),
            "pptx" => Some(OfficeFormat::Pptx),
            "html" | "htm" => Some(OfficeFormat::Html),
            "rtf" => Some(OfficeFormat::Rtf),
            "md" | "markdown" => Some(OfficeFormat::Markdown),
            _ => None,
        }
    }
}

/// A Word, PowerPoint, HTML, RTF or Markdown export ([`crate::Document::export_office_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfficeExport {
    /// The exported file.
    pub bytes: Vec<u8>,
    /// Pages (0-based) that draw forms nested deeper than [`MAX_FORM_DEPTH`] or more of them
    /// than [`MAX_FORM_VISITS`], or pictures past [`MAX_EXPORT_IMAGE_BYTES`] or past what one export
    /// may decode: export stops reading there to stay bounded on hostile files, so the text and images of those forms, or
    /// those pictures, are missing from `bytes`.
    pub truncated_pages: Vec<usize>,
}

/// An image object prepared for Office export: its file extension and encoded bytes, or why it is
/// left out (`Err(true)`: past [`MAX_EXPORT_IMAGE_BYTES`], or what the export may decode;
/// `Err(false)`: it can't be encoded).
type PreparedImage = Result<(&'static str, Arc<[u8]>), bool>;

/// Displayed points per user-space unit on `info`'s page (`/UserUnit`), 1 when the page has no
/// usable size.
pub(crate) fn display_unit(info: &pdfcraft_render::PageInfo) -> f64 {
    let crop_width = f64::from(info.crop[2] - info.crop[0]);
    let display_width = f64::from(if info.rotation.is_multiple_of(180) { info.width } else { info.height });
    if crop_width > 0.0 && crop_width.is_finite() && display_width.is_finite() { display_width / crop_width } else { 1.0 }
}

/// Bold and italic as a font's name says them (`Arial-BoldItalicMT`).
pub(crate) fn weight_and_slant(base_font: &str) -> (bool, bool) {
    let f = base_font.to_ascii_lowercase();
    (f.contains("bold") || f.contains("black") || f.contains("heavy"), f.contains("italic") || f.contains("oblique"))
}

impl crate::Document {
    /// The pages as paragraphs and images (for Word, HTML and RTF export), including what
    /// form XObjects draw.
    pub fn export_pages(&self) -> Vec<pdfcraft_export::Page> {
        self.export_pages_checked(true, MAX_EXPORT_IMAGE_BYTES).0
    }

    /// [`Self::export_pages`], and the pages whose forms were left partly unread or whose
    /// pictures went past `image_budget` bytes ([`OfficeExport::truncated_pages`]). Without
    /// `images` no picture is prepared.
    pub(crate) fn export_pages_checked(&self, images: bool, image_budget: usize) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(editor) = self.editor.as_ref() else { return (Vec::new(), Vec::new()) };
        self.export_pages_within(images, image_budget, &mut pdfcraft_cos::ImageBudget::new(&editor.cos))
    }

    /// [`Self::export_pages_checked`], its pictures decoded within `image_work`, which is shared
    /// by the whole export: a picture past it is left out like one past `image_budget`.
    pub(crate) fn export_pages_within(
        &self,
        images: bool,
        image_budget: usize,
        image_work: &mut pdfcraft_cos::ImageBudget,
    ) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(cos) = self.editor.as_ref().map(|e| &e.cos) else { return (Vec::new(), Vec::new()) };
        let mut truncated = Vec::new();
        // Each image object is encoded once and shared by every place that draws it.
        let mut encoded: HashMap<ObjRef, PreparedImage> = HashMap::new();
        let mut image_bytes = image_budget;
        let pages = self
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(i, info)| {
                // Export uses the visible page in points, y up. Raw PDF coordinates include
                // CropBox offsets and precede /Rotate and /UserUnit.
                let visible_rect = |r: [f64; 4]| {
                    let a = info.user_to_view(r[0] as f32, r[1] as f32);
                    let b = info.user_to_view(r[2] as f32, r[3] as f32);
                    [a[0].min(b[0]) as f64, (info.height - a[1].max(b[1])) as f64, a[0].max(b[0]) as f64, (info.height - a[1].min(b[1])) as f64]
                };
                let unit = display_unit(info);
                let reading = pdfcraft_edit::reading_blocks_checked(cos, i).unwrap_or_default();
                let forms_unread = reading.truncated;
                let blocks = reading
                    .blocks
                    .into_iter()
                    .filter(|b| !b.text.trim().is_empty())
                    .map(|b| {
                        let (bold, italic) = weight_and_slant(&b.base_font);
                        // `text` is trimmed; the editor text keeps the source's own spaces.
                        let space_around = [b.editor_text.starts_with(char::is_whitespace), b.editor_text.ends_with(char::is_whitespace)];
                        pdfcraft_export::Block {
                            space_around,
                            text: b.text,
                            rect: visible_rect(b.rect),
                            size: b.size * unit,
                            bold,
                            italic,
                            color: b.color,
                            rotation: b.rotation - f64::from(info.rotation),
                        }
                    })
                    .collect();
                let placed = if images { pdfcraft_edit::reading_images(cos, i).unwrap_or_default() } else { Vec::new() };
                let mut over_budget = false;
                let images = placed
                    .iter()
                    .filter_map(|im| {
                        let object = im.object?;
                        let prepared = encoded.entry(object).or_insert_with(|| {
                            let refused = image_work.refused();
                            let (ext, bytes) =
                                pdfcraft_create::image_file_within(cos, object, &mut *image_work).map_err(|_| image_work.refused() > refused)?;
                            image_bytes = image_bytes.checked_sub(bytes.len()).ok_or(true)?;
                            Ok((if ext == "jpg" { "jpg" } else { "png" }, Arc::from(bytes)))
                        });
                        over_budget |= matches!(prepared, Err(true));
                        let (ext, bytes) = prepared.clone().ok()?;
                        Some(pdfcraft_export::Image {
                            ext,
                            bytes,
                            rect: visible_rect(im.rect),
                            size: [im.matrix[0].hypot(im.matrix[1]) * unit, im.matrix[2].hypot(im.matrix[3]) * unit],
                            rotation: im.matrix[1].atan2(im.matrix[0]).to_degrees() - f64::from(info.rotation),
                        })
                    })
                    .collect();
                if forms_unread || over_budget {
                    truncated.push(i);
                }
                // The page colour (Edit a PDF ▸ Background, a producer's page fill) for formats that keep one.
                let background = pdfcraft_edit::page_background(cos, i).ok().flatten();
                pdfcraft_export::Page { width: info.width as f64, height: info.height as f64, blocks, images, background }
            })
            .collect();
        (pages, truncated)
    }

    /// The document as a Word, PowerPoint, HTML or RTF file.
    pub fn export_office(&self, format: OfficeFormat) -> Vec<u8> {
        self.export_office_checked(format).bytes
    }

    /// [`Self::export_office`], and the pages whose text it had to leave out
    /// ([`OfficeExport::truncated_pages`]).
    pub fn export_office_checked(&self, format: OfficeFormat) -> OfficeExport {
        // RTF and Markdown leave pictures out.
        let (pages, truncated_pages) =
            self.export_pages_checked(!matches!(format, OfficeFormat::Rtf | OfficeFormat::Markdown), MAX_EXPORT_IMAGE_BYTES);
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        let bytes = match format {
            OfficeFormat::Docx => pdfcraft_export::docx(&pages, &title),
            OfficeFormat::Pptx => pdfcraft_export::pptx(&pages, &title),
            OfficeFormat::Html => pdfcraft_export::html(&pages, &title).into_bytes(),
            OfficeFormat::HtmlSimple => pdfcraft_export::html_simple(&pages, &title).into_bytes(),
            OfficeFormat::Rtf => pdfcraft_export::rtf(&pages).into_bytes(),
            OfficeFormat::Markdown => pdfcraft_export::markdown(&pages).into_bytes(),
        };
        OfficeExport { bytes, truncated_pages }
    }
}
