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

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

/// Merge word boxes into rectangles, keeping each rectangle on a single page. Rectangles are
/// never merged across a page boundary, so a change that spans pages keeps one rectangle per
/// page instead of attaching later-page rectangles to the first page.
fn rects_for_words(words: &[pdfcraft_compare::Word]) -> (Vec<[f64; 4]>, Vec<usize>) {
    let mut out: Vec<[f64; 4]> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    for w in words {
        let r = w.rect;
        if let Some(last) = out.last_mut() {
            let last_page = pages.last().copied().unwrap_or(w.page);
            if last_page == w.page
                && (last[1] - r[1]).abs() < (r[3] - r[1]).max(1.0) * 0.5
                && r[0] - last[2] < (r[3] - r[1]).max(4.0) * 2.0
                && r[0] >= last[0] - 1.0
            {
                last[0] = last[0].min(r[0]);
                last[1] = last[1].min(r[1]);
                last[2] = last[2].max(r[2]);
                last[3] = last[3].max(r[3]);
                continue;
            }
        }
        out.push(r);
        pages.push(w.page);
    }
    (out, pages)
}

fn rects_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

/// The first run of `text`'s words at or after `from` that reads as `text` and, for a change
/// with a position, sits where the change was found: on its page and inside its rectangles (or
/// its anchor). `None` when the document holds no such run.
fn find_run(words: &[pdfcraft_compare::Word], text: &str, from: usize, page: usize, rects: &[[f64; 4]], near: Option<[f64; 4]>) -> Option<usize> {
    let wanted: Vec<&str> = text.split_whitespace().collect();
    let count = wanted.len();
    if count == 0 || count > words.len() || from > words.len() - count {
        return None;
    }
    let positional = !rects.is_empty() || near.is_some();
    (from..=words.len() - count).find(|&start| {
        let run = &words[start..start + count];
        run.iter().zip(&wanted).all(|(word, text)| word.text == *text)
            && run[0].page == page
            && (!positional
                || if rects.is_empty() {
                    near.is_some_and(|anchor| rects_overlap(run[0].rect, anchor))
                } else {
                    run.iter().all(|word| rects.iter().any(|rect| rects_overlap(word.rect, *rect)))
                })
    })
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
        self.words_on_pages(&[])
    }

    /// [`Self::words`] of the 0-based `pages` only (every page when empty): pages not
    /// selected are skipped before any text is extracted from them.
    pub fn words_on_pages(&self, pages: &[usize]) -> Vec<pdfcraft_compare::Word> {
        let config = pdfcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            if !pages.is_empty() && !pages.contains(&page) {
                continue;
            }
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
        self.compare_pages(old, new, &[])
    }

    /// [`Session::compare`], restricted to the 0-based `pages` (every page when empty):
    /// words on other pages take no part in the diff.
    pub fn compare_pages(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        a.check_copying()?;
        b.check_copying()?;
        let (old_words, new_words) = (a.words_on_pages(pages), b.words_on_pages(pages));
        let mut c = pdfcraft_compare::compare(&old_words, &new_words);
        let new_pages = b.info.pages.len();
        for ch in &mut c.changes {
            // A deletion keeps the newer document's own anchor when it has one. Otherwise it goes
            // on the old page when the newer document has that page, or else on the newer
            // document's last page, at its top left.
            if ch.new.rects.is_empty() && ch.new.near.is_none() {
                let page = ch.old.page.min(new_pages.saturating_sub(1));
                ch.new.page = page;
                ch.new.near = if ch.old.page == page { ch.old.rects.first().copied() } else { None };
                if ch.new.near.is_none() {
                    ch.new.near = b.info.pages.get(page).map(|info| info.crop.map(f64::from));
                }
            }
            if ch.old.rects.is_empty() {
                ch.old.page = ch.new.page;
                if ch.old.near.is_none() {
                    ch.old.near = ch.new.rects.first().copied();
                }
            }
        }
        Ok(c)
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`). Every shared page is
    /// compared when `pages` is empty, otherwise only the selected pages are rendered.
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        self.compare_visual_on_pages(old, new, dpi, &[])
    }

    /// [`Session::compare_visual`] of the 0-based `pages` only (every shared page when
    /// empty): pages not selected are skipped before any of them is rendered.
    pub fn compare_visual_on_pages(&self, old: DocId, new: DocId, dpi: f32, pages: &[usize]) -> Result<Vec<(usize, [f64; 4])>, EditError> {
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
            if !pages.is_empty() && !pages.contains(&page) {
                continue;
            }
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
        let old_words = {
            let a = self.get(old).ok_or(EditError::NoDocument)?;
            a.words_on_pages(pages)
        };
        let new_words = {
            let b = self.get(new).ok_or(EditError::NoDocument)?;
            b.words_on_pages(pages)
        };
        let new_page_crops: Vec<Option<[f64; 4]>> =
            self.get(new).map(|d| d.info.pages.iter().map(|info| Some(info.crop.map(f64::from))).collect()).unwrap_or_default();
        let new_pages = new_page_crops.len();
        // `compare` groups consecutive edits into one `Change`, and a change's new-side
        // words are a contiguous run of `new_words` with unchanged words between changes.
        // Track where the next run may start instead of counting only changed words, so
        // unchanged words before a change are not mistaken for its words.
        let mut new_search_from = 0usize;
        let mut old_search_from = 0usize;
        let mut edits = Vec::new();
        for ch in &c.changes {
            let color = colour(ch.kind);
            let contents = match ch.kind {
                Kind::Replaced => format!("Replaced: \"{}\" with \"{}\"", ch.old.text, ch.new.text),
                Kind::Inserted => format!("Inserted: \"{}\"", ch.new.text),
                Kind::Deleted => format!("Deleted: \"{}\"", ch.old.text),
            };
            // `compare` groups consecutive edits into one `Change`, and a change's new-side
            // words are a contiguous run of `new_words` with unchanged words between changes:
            // search on from the previous change's end first, then from the start.
            let new_slice = match find_run(&new_words, &ch.new.text, new_search_from, ch.new.page, &ch.new.rects, ch.new.near)
                .or_else(|| find_run(&new_words, &ch.new.text, 0, ch.new.page, &ch.new.rects, ch.new.near))
            {
                Some(start) => {
                    let count = ch.new.text.split_whitespace().count();
                    new_search_from = (start + count).min(new_words.len());
                    &new_words[start..start + count]
                }
                // The change's words aren't in the document as words (spacing, ligatures): the
                // rectangles the diff found are used as they are.
                None => &new_words[0..0],
            };

            let old_word_count = ch.old.text.split_whitespace().count();
            let mut old_slice = None;
            if old_word_count > 0 && old_word_count <= old_words.len() {
                let found = find_run(&old_words, &ch.old.text, old_search_from, ch.old.page, &ch.old.rects, ch.old.near)
                    .or_else(|| find_run(&old_words, &ch.old.text, 0, ch.old.page, &ch.old.rects, ch.old.near));
                if let Some(start) = found {
                    old_search_from = (start + old_word_count).min(old_words.len());
                    if ch.kind == Kind::Deleted {
                        old_slice = Some(&old_words[start..start + old_word_count]);
                    }
                }
            }

            if ch.kind == Kind::Deleted {
                if let Some(slice) = old_slice {
                    let mut pages_in_order: Vec<usize> = Vec::new();
                    for w in slice {
                        if !pages_in_order.contains(&w.page) {
                            pages_in_order.push(w.page);
                        }
                    }
                    for page in pages_in_order {
                        let first_rect = slice.iter().filter(|w| w.page == page).map(|w| w.rect).next();
                        let target_page = if page < new_pages { page } else { new_pages.saturating_sub(1) };
                        let near = if page == target_page { first_rect } else { None };
                        let near = near.or_else(|| new_page_crops.get(target_page).copied().flatten());
                        let Some(r) = near else { continue };
                        let shape = Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note };
                        let mut style = crate::Style::default_for(&shape);
                        style.color = color;
                        edits.push(Edit::AddAnnotation(NewAnnotation {
                            page: target_page,
                            shape,
                            style,
                            contents: contents.clone(),
                            author: "Compare".into(),
                        }));
                    }
                    continue;
                }

                let mut pages_in_order: Vec<usize> =
                    if ch.old.page <= ch.old.last_page { (ch.old.page..=ch.old.last_page).collect() } else { vec![ch.old.page] };
                pages_in_order.retain(|p| pages.is_empty() || pages.contains(p));
                pages_in_order.sort();
                pages_in_order.dedup();
                if pages_in_order.is_empty() {
                    pages_in_order.push(ch.old.page);
                }
                for page in pages_in_order {
                    let target_page = if page < new_pages { page } else { new_pages.saturating_sub(1) };
                    let first_rect = if page == target_page { new_words.iter().find(|w| w.page == target_page).map(|w| w.rect) } else { None };
                    let near = first_rect.or_else(|| new_page_crops.get(target_page).copied().flatten());
                    let Some(r) = near else { continue };
                    let shape = Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note };
                    let mut style = crate::Style::default_for(&shape);
                    style.color = color;
                    edits.push(Edit::AddAnnotation(NewAnnotation {
                        page: target_page,
                        shape,
                        style,
                        contents: contents.clone(),
                        author: "Compare".into(),
                    }));
                }
                continue;
            }
            if ch.new.rects.is_empty() {
                let Some(r) = ch.new.near else { continue };
                let shape = Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note };
                let mut style = crate::Style::default_for(&shape);
                style.color = color;
                edits.push(Edit::AddAnnotation(NewAnnotation { page: ch.new.page, shape, style, contents, author: "Compare".into() }));
                continue;
            }
            // A change can span several pages, and its rectangles keep their own pages. Re-derive
            // the rectangles from the change's words so later-page rectangles are not attached to
            // the first page or lost.
            let (rects, rect_pages) = rects_for_words(new_slice);
            let (rects, rect_pages) =
                if rects.is_empty() { (ch.new.rects.clone(), vec![ch.new.page; ch.new.rects.len()]) } else { (rects, rect_pages) };
            let mut pages_in_order: Vec<usize> = Vec::new();
            for page in &rect_pages {
                if !pages_in_order.contains(page) {
                    pages_in_order.push(*page);
                }
            }
            for page in pages_in_order {
                let rects: Vec<[f64; 4]> = rects.iter().zip(&rect_pages).filter(|(_, p)| **p == page).map(|(r, _)| *r).collect();
                let shape = Shape::TextMarkup {
                    kind: Markup::Highlight,
                    quads: rects.iter().map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]).collect(),
                };
                let mut style = crate::Style::default_for(&shape);
                style.color = color;
                edits.push(Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: contents.clone(), author: "Compare".into() }));
            }
        }
        let n = edits.len();
        if n > 0 {
            self.apply(new, Edit::Batch { label: "Mark differences".into(), edits })?;
        }
        Ok(n)
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
    /// those pictures, are missing from `bytes`. RTF also lists the pages that lost a picture to
    /// its own limits (it repeats a picture's data at every placement) or one whose rotation it
    /// couldn't bake in; their other content is kept. A picture no format can encode (a JPX image,
    /// say) is left out of every format without being listed.
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
                // The ruled lines tell a drawn table from text that merely lines up (#5149).
                let rules = pdfcraft_edit::page_rules(cos, i).unwrap_or_default().into_iter().map(visible_rect).collect();
                pdfcraft_export::Page { width: info.width as f64, height: info.height as f64, blocks, images, background, rules }
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
        // Markdown leaves pictures out.
        let (pages, mut truncated_pages) = self.export_pages_checked(!matches!(format, OfficeFormat::Markdown), MAX_EXPORT_IMAGE_BYTES);
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        // Word and HTML name the document's language (`/Lang`), so spelling is checked in it.
        let language = self.initial_view().language;
        let bytes = match format {
            OfficeFormat::Docx => pdfcraft_export::docx_with_language(&pages, &title, language.as_deref()),
            OfficeFormat::Pptx => pdfcraft_export::pptx(&pages, &title),
            OfficeFormat::Html => pdfcraft_export::html_with_language(&pages, &title, language.as_deref()).into_bytes(),
            OfficeFormat::HtmlSimple => pdfcraft_export::html_simple_with_language(&pages, &title, language.as_deref()).into_bytes(),
            OfficeFormat::Rtf => {
                let (rtf, omitted) = pdfcraft_export::rtf_with_title_checked(&pages, &title);
                truncated_pages.extend(omitted);
                truncated_pages.sort_unstable();
                truncated_pages.dedup();
                rtf.into_bytes()
            }
            OfficeFormat::Markdown => pdfcraft_export::markdown(&pages).into_bytes(),
        };
        OfficeExport { bytes, truncated_pages }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_differences_marks_each_occurrence() {
        let build = |second: &str| -> Vec<u8> {
            let first_text = "Rate 950";
            let second_text = second;
            let c1 = format!("BT /F1 12 Tf 100 700 Td ({first_text}) Tj ET");
            let c2 = format!("BT /F1 12 Tf 100 700 Td ({second_text}) Tj ET");
            let o1 = "<< /Type /Catalog /Pages 2 0 R >>".to_string();
            let o2 = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string();
            let o3 = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".to_string();
            let o4 = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".to_string();
            let o5 = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string();
            let o6 = format!("<< /Length {} >>\nstream\n{}\nendstream", c1.len(), c1);
            let o7 = format!("<< /Length {} >>\nstream\n{}\nendstream", c2.len(), c2);
            let objects = [o1, o2, o3, o4, o5, o6, o7];
            let mut pdf = String::new();
            pdf.push_str("%PDF-1.4\n");
            let mut offsets = Vec::new();
            for obj in &objects {
                offsets.push(pdf.len());
                pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", offsets.len(), obj));
            }
            let xref = pdf.len();
            pdf.push_str("xref\n0 8\n0000000000 65535 f \n");
            for off in offsets {
                pdf.push_str(&format!("{:010} 00000 n \n", off));
            }
            pdf.push_str(&format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", xref));
            pdf.into_bytes()
        };

        let old_bytes = build("Rate 900");
        let new_bytes = build("Rate 950");

        let mut session = Session::new();
        let old = session.open("old.pdf", None, Arc::new(old_bytes), None).unwrap();
        let new = session.open("new.pdf", None, Arc::new(new_bytes), None).unwrap();

        let marked = session.mark_differences_of(old, new, &[]).unwrap();
        assert_eq!(marked, 1);

        let doc = session.get(new).unwrap();
        assert!((0..2).any(|i| doc.comment_props(1, i).is_some()));
        assert!((0..2).all(|i| doc.comment_props(0, i).is_none()));
    }
}
