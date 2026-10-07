//! Compare files: the text differences between two open documents, a report, and the
//! differences marked as comments in the newer one.

use std::sync::Arc;

pub use printcraft_compare::{Change, Comparison, Kind, Side};

use crate::{DocId, Edit, EditError, Markup, NewAnnotation, NoteIcon, Session, Shape};

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
#[must_use]
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

impl crate::Document {
    /// Standards ▸ Verify PDF/A: the rules the document breaks for `level`.
    pub fn pdfa_verify(&self, level: printcraft_preflight::Level) -> Vec<printcraft_preflight::Issue> {
        self.editor.as_ref().map(|e| printcraft_preflight::verify(&e.cos, level)).unwrap_or_default()
    }

    /// The standards the document declares (Standards panel).
    pub fn standards(&self) -> printcraft_preflight::Declared {
        self.editor.as_ref().map(|e| printcraft_preflight::declared(&e.cos)).unwrap_or_default()
    }

    /// Every word of the document in reading order, with page and box.
    pub fn words(&self) -> Vec<printcraft_compare::Word> {
        let config = printcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = printcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            let res =
                r.render(printcraft_render::RenderRequest { page, kind: printcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(t) = res.text {
                out.extend(crate::js::page_words(&t, info).into_iter().map(|(text, rect)| printcraft_compare::Word { text, page, rect }));
            }
        }
        out
    }
}

impl Session {
    /// Compare files: the text differences from document `old` to document `new`.
    ///
    /// # Errors
    ///
    /// When `old` or `new` is not an open document (`EditError::NoDocument`).
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        Ok(printcraft_compare::compare(&a.words(), &b.words()))
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`).
    ///
    /// # Errors
    ///
    /// When `old` or `new` is not an open document (`EditError::NoDocument`). Pages that fail to
    /// render are skipped, not reported.
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        let first = self.get(old).ok_or(EditError::NoDocument)?;
        let second = self.get(new).ok_or(EditError::NoDocument)?;
        let renderer = |doc: &crate::Document| {
            printcraft_render::PageRenderer::new(
                doc.bytes.clone(),
                printcraft_render::RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() },
            )
        };
        let (mut ra, mut rb) = (renderer(first), renderer(second));
        let scale = dpi.clamp(18.0, 150.0) / 72.0;
        let mut out = Vec::new();
        for page in 0..first.info.pages.len().min(second.info.pages.len()) {
            let req = printcraft_render::RenderRequest { page, scale, ..Default::default() };
            let (old_page, new_page) = (ra.render(req), rb.render(req));
            if old_page.error.is_some() || new_page.error.is_some() {
                continue;
            }
            let info = &second.info.pages[page];
            let view_scale = new_page.width as f32 / info.width.max(1e-3);
            let regions = printcraft_compare::visual_regions(
                (&new_page.rgba, new_page.width, new_page.height),
                (&old_page.rgba, old_page.width, old_page.height),
                24,
            );
            for region in regions {
                let p = info.view_to_user(region[0] as f32 / view_scale, region[1] as f32 / view_scale);
                let q = info.view_to_user(region[2] as f32 / view_scale, region[3] as f32 / view_scale);
                out.push((page, [f64::from(p[0].min(q[0])), f64::from(p[1].min(q[1])), f64::from(p[0].max(q[0])), f64::from(p[1].max(q[1]))]));
            }
        }
        Ok(out)
    }

    /// The compare report as a new PDF (not opened).
    ///
    /// # Errors
    ///
    /// When `old` or `new` is not an open document (`EditError::NoDocument`), the report text
    /// cannot be laid out (`EditError::Create`), or writing the new PDF fails
    /// (`EditError::Write`).
    pub fn compare_report(&self, old: DocId, new: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let c = self.compare(old, new)?;
        let name = |id| self.get(id).map(|d| d.name.clone()).unwrap_or_default();
        self.create_from_text("Compare Report", &printcraft_compare::report(&c, &name(old), &name(new)))
    }

    /// Mark the differences in `new` as comments: highlights over replaced and inserted text
    /// (blue, green) and a note where text was deleted (red), authored "Compare". One undoable
    /// step; returns how many comments were added.
    ///
    /// # Errors
    ///
    /// When `old` or `new` is not an open document (`EditError::NoDocument`), or adding the
    /// comments fails: the document is read-only, its security settings refuse annotations
    /// (`EditError::NotPermitted`), or the result cannot be written and re-opened (see
    /// [`Session::apply`]).
    pub fn mark_differences(&mut self, old: DocId, new: DocId) -> Result<usize, EditError> {
        let c = self.compare(old, new)?;
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

/// Export a PDF ▸ Word, HTML or RTF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    Docx,
    Html,
    Rtf,
}

impl OfficeFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Html => "html",
            OfficeFormat::Rtf => "rtf",
        }
    }

    #[must_use]
    pub fn from_extension(ext: &str) -> Option<OfficeFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some(OfficeFormat::Docx),
            "html" | "htm" => Some(OfficeFormat::Html),
            "rtf" => Some(OfficeFormat::Rtf),
            _ => None,
        }
    }
}

impl crate::Document {
    /// The pages as paragraphs and images (for Word, HTML and RTF export).
    pub fn export_pages(&self) -> Vec<printcraft_export::Page> {
        let Some(cos) = self.editor.as_ref().map(|e| &e.cos) else { return Vec::new() };
        self.info
            .pages
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let blocks = printcraft_edit::text_blocks(cos, i)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|b| !b.text.trim().is_empty())
                    .map(|b| {
                        let f = b.base_font.to_ascii_lowercase();
                        printcraft_export::Block {
                            text: b.text,
                            rect: b.rect,
                            size: b.size,
                            bold: f.contains("bold") || f.contains("black") || f.contains("heavy"),
                            italic: f.contains("italic") || f.contains("oblique"),
                        }
                    })
                    .collect();
                let images = self
                    .page_images(i)
                    .iter()
                    .enumerate()
                    .filter_map(|(k, im)| {
                        let (ext, bytes) = self.page_image_file(i, k).ok()?;
                        Some(printcraft_export::Image { ext: if ext == "jpg" { "jpg" } else { "png" }, bytes, rect: im.rect })
                    })
                    .collect();
                printcraft_export::Page { width: f64::from(info.width), height: f64::from(info.height), blocks, images }
            })
            .collect()
    }

    /// The document as a Word, HTML or RTF file.
    pub fn export_office(&self, format: OfficeFormat) -> Vec<u8> {
        let pages = self.export_pages();
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        match format {
            OfficeFormat::Docx => printcraft_export::docx(&pages, &title),
            OfficeFormat::Html => printcraft_export::html(&pages, &title).into_bytes(),
            OfficeFormat::Rtf => printcraft_export::rtf(&pages).into_bytes(),
        }
    }
}
