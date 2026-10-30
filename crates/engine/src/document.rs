//! An open document: its working file, view data and edit state.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_render::{DocInfo, RenderConfig, RenderPool};

use crate::history::Editor;
use crate::{
    CalculationOrder, CheckStyle, DocId, Edit, EditError, EditableObject, FormFieldKind, Hidden, InitialView, Label, MarkKind, SecuritySummary,
    SignatureInfo, TabOrder, TrustStore, XfaLayout, a11y, form_appearance, guard, js, measure, optimize, renderer_password, unrestricted,
};
#[cfg(test)]
use crate::{record_link_scan, record_marker_scan};

pub struct Document {
    pub id: DocId,
    pub name: String,
    pub path: Option<String>,
    /// The working file: what Save writes.
    pub bytes: Arc<Vec<u8>>,
    /// What is displayed (the page view, `page_render`, page images): `bytes`, plus appearances
    /// drawn for display only for comments that have none ([`display_bytes`]). Never saved.
    pub display: Arc<Vec<u8>>,
    pub info: DocInfo,
    pub renderer: RenderPool,
    /// The password the document was opened with (needed to read attachments, etc.).
    pub password: Option<String>,
    /// `true` when there are edits that have not been saved.
    pub dirty: bool,
    /// Bumped on every change to the working file (edit, undo, redo, save); autosave compares it.
    pub(crate) generation: u64,
    /// The generation last handed out by `autosave_snapshots`.
    pub(crate) snapshot_generation: u64,
    /// Why the document cannot be edited (e.g. encryption), if so.
    pub read_only_reason: Option<String>,
    /// Interactive form fields of the current state (empty without a form).
    pub form: Arc<Vec<pdfcraft_forms::Field>>,
    pub(crate) marker_scan: OnceLock<pdfcraft_edit::MarkerScan>,
    pub(crate) links: OnceLock<Vec<pdfcraft_annot::links::LinkItem>>,
    /// Signature fields, validated against the session's trust store (Signatures panel).
    pub signatures: Arc<Vec<SignatureInfo>>,
    pub(crate) trust: Arc<TrustStore>,
    pub(crate) sig_cache: Arc<pdfcraft_sign::DigestCache>,
    pub(crate) editor: Option<Editor>,
    pub(crate) config: RenderConfig,
    /// What field scripts printed or asked for (see [`Session::take_js_output`]).
    pub(crate) js_output: js::JsOutput,
    /// Dynamic XFA forms: what laying the template out produced (pages and fields are
    /// PdfCraft's; Adobe's viewers draw the form from the XFA packets themselves).
    pub xfa: Option<XfaLayout>,
    /// The parsed template of a laid-out XFA form, for its scripts.
    pub(crate) xfa_template: Option<Arc<pdfcraft_xfa::model::Template>>,
    /// A script of the XFA form ran away, so its scripts are off (see [`Document::xfa_scripts_off`]).
    pub(crate) xfa_scripts_off: bool,
    /// A content-only refresh keeps `info` from before the edit; an edit may have added or
    /// embedded fonts, so the font list is read again on demand ([`Session::refresh_fonts`]).
    pub(crate) fonts_stale: bool,
    /// XFA forms: what was approximated, rewritten or could not be written to the XFA data
    /// (also in `info.warnings`, kept there when the document is re-read).
    pub xfa_warnings: Vec<String>,
    /// What the last successful [`Session::apply`](crate::Session::apply) wants said (see
    /// [`EditNotice`]): text its fonts can't draw, pages whose own backdrop stays off. The
    /// interface shows it; tools return it.
    pub edit_notices: Vec<EditNotice>,
}

/// Something the last successful edit did that the user should hear about, never left silent
/// (AGENTS.md §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditNotice {
    /// Text stored as typed but drawn with `?`.
    Undrawn(TextNotice),
    /// Remove background couldn't turn back on the white backdrop these pages (0-based,
    /// ascending) paint themselves: their content changed after the background replaced it, so
    /// turning it on could paint over something (`pdfcraft_edit::backdrops_left_off`). They look
    /// the same on white paper.
    BackdropsLeftOff { pages: Vec<usize> },
}

impl std::fmt::Display for EditNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditNotice::Undrawn(n) => n.fmt(f),
            EditNotice::BackdropsLeftOff { pages } => {
                let (which, own) = if pages.len() == 1 { ("page", "its") } else { ("pages", "their") };
                write!(
                    f,
                    "{which} {} changed after the background was added, so {own} own white backdrop was not turned back on (it looks the same on white paper)",
                    page_runs(pages)
                )
            }
        }
    }
}

/// 0-based pages as 1-based runs: `[0, 1, 2, 5]` is "1-3, 6".
pub(crate) fn page_runs(pages: &[usize]) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &p in pages {
        match runs.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(p) => *end = p,
            _ => runs.push((p, p)),
        }
    }
    let one = |p: usize| p.saturating_add(1);
    runs.iter().map(|&(a, b)| if a == b { one(a).to_string() } else { format!("{}-{}", one(a), one(b)) }).collect::<Vec<_>>().join(", ")
}

/// Text an edit stored as typed but drew with `?` in place of `ch`, because the font it is drawn
/// with (a form field's, or the standard fonts for comments) can't draw that character. Acrobat
/// keeps such text too, drawing it with fonts PdfCraft can't embed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextNotice {
    /// The form field's full name, or `None` for a comment.
    pub field: Option<String>,
    /// The page (0-based) of the comment or of the field's widget.
    pub page: usize,
    pub ch: char,
    /// Other PDF viewers redraw it from the stored text (a form field, with `/NeedAppearances`
    /// set); otherwise the `?` shows in every viewer.
    pub redrawn_elsewhere: bool,
}

impl std::fmt::Display for TextNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ch = format!("\"{}\" (U+{:04X})", self.ch.escape_debug(), u32::from(self.ch));
        let page = self.page + 1;
        match (&self.field, self.redrawn_elsewhere) {
            (Some(name), true) => write!(
                f,
                "form field {name:?} (page {page}) holds {ch}, which its font can't draw: the value is saved, PdfCraft shows ? in its place, and other PDF viewers redraw the field from the value"
            ),
            (Some(name), false) => write!(
                f,
                "form field {name:?} (page {page}) holds {ch}, which its font can't draw: the value is saved, but this PDF/A document can't ask other viewers to redraw it, so ? shows in its place everywhere"
            ),
            (None, _) => write!(
                f,
                "a comment on page {page} has {ch}, which the standard fonts can't draw: its text is saved, but its appearance shows ? in its place in every viewer"
            ),
        }
    }
}

/// What `next` drew with `?` that `prev` didn't (a new or redrawn appearance carrying
/// `pdfcraft_fonts::UNDRAWN_MARK`), one notice per field or comment, in page order.
pub(crate) fn undrawn_notices(prev: &pdfcraft_cos::Document, next: &pdfcraft_cos::Document) -> Vec<TextNotice> {
    let Ok(pages) = pdfcraft_annot::page_refs(next) else { return Vec::new() };
    let redrawn_elsewhere = pdfcraft_forms::need_appearances(next);
    let mut names: Option<std::collections::HashMap<pdfcraft_cos::ObjRef, String>> = None;
    let mut out: Vec<TextNotice> = Vec::new();
    for (page, p) in pages.iter().enumerate() {
        let annots = next.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| next.resolve(&a));
        for r in annots.as_deref().and_then(pdfcraft_cos::Object::as_array).into_iter().flatten().filter_map(pdfcraft_cos::Object::as_ref) {
            let now = next.get(r);
            let Some(d) = now.as_dict() else { continue };
            let Some(ch) = pdfcraft_fonts::undrawn_mark(d) else { continue };
            // Only what this edit drew: a new annotation, or a new appearance.
            let was = prev.get(r);
            if was.as_dict().is_some_and(|w| w.get(b"AP") == d.get(b"AP") && pdfcraft_fonts::undrawn_mark(w) == Some(ch)) {
                continue;
            }
            let field = (d.name(b"Subtype") == Some(b"Widget")).then(|| {
                let names = names.get_or_insert_with(|| {
                    pdfcraft_forms::fields(next)
                        .into_iter()
                        .flat_map(|f| f.widgets.iter().map(|w| (w.obj, f.name.clone())).collect::<Vec<_>>())
                        .collect()
                });
                names.get(&r).cloned().unwrap_or_default()
            });
            if field.is_some() && out.iter().any(|n| n.field == field) {
                continue;
            }
            out.push(TextNotice { redrawn_elsewhere: redrawn_elsewhere && field.is_some(), field, page, ch });
        }
    }
    out
}

impl Document {
    /// How the document opens (Document Properties ▸ Initial View).
    /// Accessibility ▸ Check for accessibility: the full check (`None` if the document can't be
    /// read for editing).
    pub fn accessibility_check(&self, options: &a11y::Options) -> Option<a11y::Report> {
        self.editor.as_ref().map(|e| a11y::check(&e.cos, options))
    }

    /// A script of this XFA form ran past its time limit and was abandoned, so the form's
    /// scripts no longer run in this document (reopening it runs them again). The script's
    /// error is in the JavaScript output; this is the state, for a viewer to show.
    pub fn xfa_scripts_off(&self) -> bool {
        self.xfa_scripts_off
    }

    /// PDF Optimizer ▸ Audit space usage.
    pub fn audit_space(&self) -> Vec<optimize::SpaceUse> {
        self.editor.as_ref().map(|e| optimize::audit_space(&e.cos, self.bytes.len() as u64)).unwrap_or_default()
    }

    /// The backdrop `page` (0-based) paints itself, which hides a background (Add Background).
    pub fn page_backdrop(&self, page: usize) -> Option<pdfcraft_edit::Backdrop> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::page_backdrop(&e.cos, page).ok().flatten())
    }

    /// [`Self::page_backdrop`] for each of `pages`, in order, walking the page tree once: the
    /// way to check many pages. A page out of range has none.
    pub fn page_backdrops(&self, pages: &[usize]) -> Vec<Option<pdfcraft_edit::Backdrop>> {
        match &self.editor {
            Some(e) => pdfcraft_edit::page_backdrops(&e.cos, pages),
            None => vec![None; pages.len()],
        }
    }

    /// Edit a PDF: the images `page` draws (0-based).
    pub fn page_images(&self, page: usize) -> Vec<pdfcraft_edit::PageImage> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::page_images(&e.cos, page).ok()).unwrap_or_default()
    }

    /// Mixed editable objects on one page. Source inventories are zero-based and added items
    /// appear once. Rectangles are user-space points, independent of the viewer's zoom.
    pub fn editable_objects(&self, page: usize) -> Result<Vec<EditableObject>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_edit::editable_objects(&editor.cos, page).map_err(|e| e.to_string())
    }

    /// Displayed top-left-origin motion to a user-space vector. Use the linear map only,
    /// so even a large crop-box origin cannot erase a small drag by cancellation.
    pub fn object_move_offset(&self, page: usize, delta: [f64; 2]) -> Result<[f64; 2], String> {
        let p = self.info.pages.get(page).ok_or("the page no longer exists")?;
        let [x0, y0, x1, y1] = p.crop.map(f64::from);
        let (a, b) = (delta[0] / f64::from(p.width.max(1e-3)), delta[1] / f64::from(p.height.max(1e-3)));
        let (u, v) = match p.rotation {
            90 => (b, -a),
            180 => (-a, -b),
            270 => (-b, a),
            _ => (a, b),
        };
        let offset = [u * (x1 - x0), -v * (y1 - y0)];
        if !offset.iter().all(|v| v.is_finite()) {
            return Err("the move must be finite and non-zero".into());
        }
        Ok(offset)
    }

    /// Save image as: image `index` on `page` as a file (extension, bytes).
    pub fn page_image_file(&self, page: usize, index: usize) -> Result<(&'static str, Vec<u8>), String> {
        self.check_copying().map_err(|e| e.to_string())?;
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        let img = self.page_images(page).into_iter().nth(index).ok_or_else(|| format!("page {} has no image {}", page + 1, index + 1))?;
        if img.is_form {
            return Err("grouped Form artwork is not a bitmap; Save Image As is only available for raster images".into());
        }
        pdfcraft_create::image_file(&editor.cos, img.object.ok_or("the image has no object")?)
    }

    /// Edit a PDF ▸ Edit text: the paragraphs on `page` (0-based).
    pub fn text_blocks(&self, page: usize) -> Vec<pdfcraft_edit::TextBlock> {
        self.editor.as_ref().and_then(|e| e.text_blocks(page).ok()).map(|b| b.as_ref().clone()).unwrap_or_default()
    }

    /// Saved measurement annotations, calculated from their geometry and PDF scales, plus
    /// the ones that couldn't be read (unsupported formats are skipped, not fatal).
    pub fn measurements(&self) -> Result<measure::Listing, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        Ok(measure::list(&e.cos))
    }
    pub fn measurement_scale(&self, page: usize, at: measure::Point) -> Result<measure::Scale, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::scale_at(&e.cos, page, at).map_err(|e| e.to_string())
    }
    /// [`Self::measurement_scale`], measuring true size in `unit` (`per_point` of them in a
    /// point) where the page sets no scale.
    pub fn measurement_scale_or(&self, page: usize, at: measure::Point, per_point: f64, unit: &str) -> Result<measure::Scale, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::scale_at_or(&e.cos, page, at, per_point, unit).map_err(|e| e.to_string())
    }
    pub fn measurement_to_user(&self, page: usize, point: measure::Point) -> Result<measure::Point, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::view_to_user(&e.cos, page, point).map_err(|e| e.to_string())
    }
    pub fn measurement_to_view(&self, page: usize, point: measure::Point) -> Result<measure::Point, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::user_to_view(&e.cos, page, point).map_err(|e| e.to_string())
    }
    pub fn measurement_paths(&self, page: usize) -> Result<measure::snap::Geometry, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::snap::geometry(&e.cos, page).map_err(|e| e.to_string())
    }

    /// A counter that changes with every edit (for caches of derived data).
    pub fn edit_generation(&self) -> u64 {
        self.generation
    }

    /// The styles the document's text is set in, the largest first, for bookmarks from text
    /// styles (#1693).
    /// Empty when the document doesn't allow copying: the samples are its text.
    pub fn text_styles(&self) -> Vec<crate::text_style_bookmarks::TextStyleUse> {
        if !self.allows_copying() {
            return Vec::new();
        }
        self.editor.as_ref().map(|e| crate::text_style_bookmarks::styles(&e.cos)).unwrap_or_default()
    }

    /// Edit a PDF ▸ Edit text: the lines of existing text on `page` (0-based).
    pub fn text_lines(&self, page: usize) -> Vec<pdfcraft_edit::TextLine> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::text_lines(&e.cos, page).ok()).unwrap_or_default()
    }

    /// The form's calculation order: its calculated fields in the order they calculate, what each
    /// formula reads, what is wrong with the order, and the order Fix order writes.
    pub fn calculation_order(&self) -> CalculationOrder {
        self.editor.as_ref().map(|e| pdfcraft_forms::calculation_order(&e.cos, &self.form)).unwrap_or_default()
    }

    /// Page properties ▸ Tab order: each page's tab order, in page order.
    pub fn tab_orders(&self) -> Vec<TabOrder> {
        self.editor.as_ref().map(|e| pdfcraft_forms::tab_orders(&e.cos)).unwrap_or_default()
    }

    /// Add alternate text: the figures, in document order.
    pub fn figures(&self) -> Vec<a11y::Figure> {
        self.editor.as_ref().map(|e| a11y::figures(&e.cos)).unwrap_or_default()
    }

    /// Reachable indirect structure tags, in document order.
    pub fn tags(&self) -> Vec<a11y::TagInfo> {
        self.editor.as_ref().map(|e| a11y::tags(&e.cos)).unwrap_or_default()
    }

    /// Marked-content items directly owned by one reachable indirect structure tag.
    pub fn tag_content(&self, object: pdfcraft_cos::ObjRef) -> Result<Vec<a11y::TagContentItem>, a11y::TagError> {
        self.editor.as_ref().map(|e| a11y::tag_content(&e.cos, object)).unwrap_or_else(|| Ok(Vec::new()))
    }

    /// The edit that fixes `rule`, for the rules with an automatic fix: the document language
    /// (`value` is the language), the title (`value` replaces it; else the current title or the
    /// file name) and the tab order.
    pub fn accessibility_fix(&self, rule: a11y::Rule, value: Option<&str>) -> Result<Edit, String> {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        match rule {
            a11y::Rule::PrimaryLanguage => {
                let mut v = self.initial_view();
                v.language = Some(value.ok_or("give the document language (for example en-US)")?.to_owned());
                Ok(Edit::SetInitialView(Box::new(v)))
            }
            a11y::Rule::Title => {
                let title = value
                    .map(str::to_owned)
                    .or_else(|| self.info_value("Title").filter(|t| !t.trim().is_empty()))
                    .unwrap_or_else(|| self.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_owned());
                let mut v = self.initial_view();
                v.display_title = true;
                Ok(Edit::Batch {
                    label: "Set document title".into(),
                    edits: vec![Edit::SetInfo { key: "Title".into(), value: title }, Edit::SetInitialView(Box::new(v))],
                })
            }
            a11y::Rule::TabOrder => Ok(Edit::SetTabOrder { pages: (0..self.info.pages.len()).collect(), order: TabOrder::Structure }),
            other => Err(format!("\"{}\" has no automatic fix", other.name())),
        }
    }

    pub fn initial_view(&self) -> InitialView {
        self.editor.as_ref().map(|e| pdfcraft_organize::initial_view(&e.cos)).unwrap_or_default()
    }

    /// Signed: at least one signature field holds a signature.
    pub fn is_signed(&self) -> bool {
        self.signatures.iter().any(|s| s.signed)
    }

    /// Whether comments are hidden (Comments ▸ Hide all comments).
    pub fn comments_hidden(&self) -> bool {
        self.config.hide_comments
    }

    /// Whether `Edit::DeleteAnnotation` would delete the annotation at `(page, index)`: the
    /// engine's own check (it, and every reply that would go with it, unlocked), for a Delete
    /// command to ask before it is offered.
    pub fn annotation_deletable(&self, page: usize, index: usize) -> bool {
        self.editor.as_ref().is_some_and(|e| pdfcraft_annot::deletable(&e.cos, page, index).is_ok())
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo_label().map(Label::as_str)
    }

    pub fn can_redo(&self) -> Option<&str> {
        self.redo_label().map(Label::as_str)
    }

    /// What Undo would undo, in parts, for an interface that says it in another language.
    pub fn undo_label(&self) -> Option<&Label> {
        self.editor.as_ref().and_then(|e| e.undo.last()).map(|(l, ..)| l)
    }

    /// What Redo would redo, in parts.
    pub fn redo_label(&self) -> Option<&Label> {
        self.editor.as_ref().and_then(|e| e.redo.last()).map(|(l, ..)| l)
    }

    pub fn editable(&self) -> bool {
        self.editor.is_some()
    }

    /// What the opening password allows; `None` when the document is not encrypted.
    pub fn permissions(&self) -> Option<pdfcraft_cos::Permissions> {
        self.editor.as_ref().and_then(|e| e.cos.permissions())
    }

    /// Printing is allowed (Table 22, bit 3).
    pub fn allows_printing(&self) -> bool {
        self.permissions().is_none_or(|p| p.print())
    }

    /// Copying or otherwise extracting text and images is allowed (Table 22, bit 5): copying
    /// selected text, snapshots, Save Image As, exporting. Assistive technology may read the
    /// content regardless (§7.6.4.2), so reading aloud and accessibility tools don't ask.
    pub fn allows_copying(&self) -> bool {
        self.permissions().is_none_or(|p| p.copy())
    }

    /// [`Self::allows_copying`], as the error to report when it isn't.
    pub fn check_copying(&self) -> Result<(), EditError> {
        if self.allows_copying() { Ok(()) } else { Err(EditError::NotPermitted("copying text and images")) }
    }

    /// Page changes (insert, delete, rotate, move, extract) are allowed.
    pub fn allows_assembly(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.assemble())
    }

    /// Redaction marks waiting to be applied.
    pub fn redaction_marks(&self) -> usize {
        self.info.annotations.iter().filter(|a| a.subtype == "Redact").count()
    }

    /// Page marks present (headers and footers, watermarks, backgrounds), for Update/Remove.
    pub fn marks(&self) -> &[MarkKind] {
        &self
            .marker_scan
            .get_or_init(|| {
                self.editor
                    .as_ref()
                    .map(|e| {
                        #[cfg(test)]
                        record_marker_scan();
                        pdfcraft_edit::scan_markers(&e.cos)
                    })
                    .unwrap_or_default()
            })
            .marks
    }

    /// Text and images added with Edit a PDF ▸ Add content (still editable).
    pub fn added(&self) -> &[pdfcraft_edit::Added] {
        &self
            .marker_scan
            .get_or_init(|| {
                self.editor
                    .as_ref()
                    .map(|e| {
                        #[cfg(test)]
                        record_marker_scan();
                        pdfcraft_edit::scan_markers(&e.cos)
                    })
                    .unwrap_or_default()
            })
            .added
    }

    /// Link annotations, for Edit a PDF ▸ Link.
    pub fn links(&self) -> &[pdfcraft_annot::links::LinkItem] {
        self.links.get_or_init(|| {
            self.editor
                .as_ref()
                .map(|e| {
                    #[cfg(test)]
                    record_link_scan();
                    pdfcraft_annot::links::list(&e.cos)
                })
                .unwrap_or_default()
        })
    }

    /// Changes to content and document information are allowed.
    pub fn allows_modification(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.modify())
    }

    /// How widget `widget` of text field `name` lays its value out, for asking whether a typed
    /// value fits it ([`form_appearance::TextBox::fits`]): a field that doesn't scroll takes no
    /// more than fits. `None` without such a field or widget, or when the document can't be
    /// edited.
    pub fn field_text_box(&self, name: &str, widget: usize) -> Option<form_appearance::TextBox> {
        let cos = &self.editor.as_ref()?.cos;
        let f = self.form.iter().find(|f| f.name == name)?;
        let w = f.widgets.get(widget)?;
        Some(form_appearance::text_box(cos, f, w))
    }

    /// Filling in form fields is allowed (Table 22, bits 6 and 9).
    pub fn allows_form_filling(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.fill_forms())
    }

    /// Adding and changing comments is allowed (Table 22, bit 6).
    pub fn allows_annotation(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.annotate())
    }

    /// The document's security may be changed (Protect, Remove security): it is editable and,
    /// if encrypted, was opened with the owner password.
    pub fn allows_security_change(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| unrestricted(&p))
    }

    /// A password restricts the document and it wasn't opened with the owner password: entering
    /// that password ([`Session::unlock`](crate::Session::unlock)) lifts the restrictions.
    pub fn can_unlock(&self) -> bool {
        self.editable() && self.permissions().is_some_and(|p| !unrestricted(&p))
    }

    /// A summary of the document's security for Document Properties ▸ Security, including
    /// protection applied in this session (written by the next save).
    pub fn security_summary(&self) -> Option<SecuritySummary> {
        let editor = self.editor.as_ref()?;
        let h = editor.cos.output_handler()?;
        let d = h.dict();
        let stream = d.crypt_filters.iter().find(|(name, _)| *name == d.stm_f).map(|(_, m)| *m);
        let method = match (d.v, stream) {
            (1..=3, _) if d.length_bits <= 40 => "RC4, 40-bit",
            (1..=3, _) | (_, Some(pdfcraft_cos::CryptMethod::Rc4)) => "RC4, 128-bit",
            (_, Some(pdfcraft_cos::CryptMethod::Aes128)) => "AES, 128-bit",
            (_, Some(pdfcraft_cos::CryptMethod::Aes256)) => "AES, 256-bit",
            _ => "Attachments only",
        };
        let pending = editor.cos.encryption_changed();
        let permissions = if pending { pdfcraft_cos::Permissions { bits: h.permissions().bits, owner: false } } else { h.permissions() };
        Some(SecuritySummary { method: method.into(), owner: h.auth() == pdfcraft_cos::Auth::Owner && !pending, permissions, pending })
    }

    /// Comment properties of the comment at `(page, index)` (Comment Properties dialog).
    pub fn comment_props(&self, page: usize, index: usize) -> Option<pdfcraft_annot::Props> {
        self.editor.as_ref().and_then(|e| pdfcraft_annot::props(&e.cos, page, index))
    }

    /// A form field's Appearance-tab look (borders, colours, font).
    pub fn field_look(&self, name: &str) -> Option<pdfcraft_forms::Look> {
        let e = self.editor.as_ref()?;
        let f = self.form.iter().find(|f| f.name == name)?;
        Some(pdfcraft_forms::look(&e.cos, f))
    }

    /// A check box's or radio button's mark (Field Properties ▸ Options), `None` for other fields.
    pub fn field_check_style(&self, name: &str) -> Option<CheckStyle> {
        let e = self.editor.as_ref()?;
        let f = self.form.iter().find(|f| f.name == name)?;
        matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio).then(|| pdfcraft_forms::check_style(&e.cos, f))
    }

    /// Whether text field `name`'s value is cut off by its box at the field's font size (#4018):
    /// the page (0-based) of the first widget that cuts it, and how.
    pub fn field_overflow(&self, name: &str) -> Option<(Option<usize>, pdfcraft_forms::appearance::Overflow)> {
        let e = self.editor.as_ref()?;
        let f = self.form.iter().find(|f| f.name == name)?;
        f.widgets.iter().find_map(|w| pdfcraft_forms::appearance::value_overflow(&e.cos, f, w).map(|o| (w.page, o)))
    }

    /// Remove Hidden Information: what each category would remove.
    pub fn hidden_info(&self) -> Vec<(Hidden, usize)> {
        self.editor.as_ref().map(|e| pdfcraft_redact::sanitize::scan(&e.cos)).unwrap_or_default()
    }

    /// Verification: faux redactions (opaque covers over extractable text) and redaction
    /// marks that were never applied. Fails closed: any read error is returned, never an
    /// empty list.
    pub fn audit_redactions(&self) -> Result<Vec<pdfcraft_audit::AuditFinding>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_audit::audit_redactions(&editor.cos).map_err(|e| e.to_string())
    }

    /// Every page's media, crop, bleed, trim and art boxes (user space), for Set Page Boxes.
    pub fn page_boxes(&self) -> Vec<[[f64; 4]; 5]> {
        self.editor.as_ref().and_then(|e| pdfcraft_organize::page_boxes(&e.cos).ok()).unwrap_or_default()
    }

    /// Current value of a document-information entry (Title, Author, …).
    pub fn info_value(&self, key: &str) -> Option<String> {
        self.editor.as_ref().and_then(|e| pdfcraft_organize::info(&e.cos, key))
    }

    /// Whether the document information has an entry `key`, of any type, listed or not.
    pub fn has_info(&self, key: &str) -> bool {
        self.editor.as_ref().is_some_and(|e| pdfcraft_organize::has_info(&e.cos, key))
    }

    /// The custom document properties as they are now (name, text), Document Properties ▸ Custom.
    pub fn custom_info(&self) -> Vec<(String, String)> {
        self.editor.as_ref().map(|e| pdfcraft_organize::custom_info(&e.cos)).unwrap_or_default()
    }

    /// The name shown on the tab and window: the document title when the document asks for it
    /// (Initial View ▸ Show: Document Title) and has a real one, else the file name.
    pub fn display_name(&self) -> String {
        self.editor
            .as_ref()
            .filter(|e| pdfcraft_organize::displays_doc_title(&e.cos))
            .and_then(|e| pdfcraft_organize::info(&e.cos, "Title"))
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty() && !is_placeholder_title(t))
            .unwrap_or_else(|| self.name.clone())
    }

    /// Where each saved revision of the file ends (oldest first; `bytes[..end]` is that
    /// revision). Unsaved edits aren't a revision yet.
    pub fn revision_ends(&self) -> Vec<usize> {
        self.editor.as_ref().map(|e| e.cos.revision_ends()).unwrap_or_default()
    }

    /// The file as last opened or saved, without unsaved edits: what an incremental save appends
    /// to. This stays that file when undo has restored an older snapshot.
    pub fn saved_bytes(&self) -> Arc<Vec<u8>> {
        self.editor.as_ref().map_or_else(|| self.bytes.clone(), |e| e.saved.clone())
    }

    /// Notes about damage repaired while opening (Document Properties ▸ Advanced, notices).
    pub fn repair_log(&self) -> Vec<String> {
        self.editor.as_ref().map(|e| e.cos.repair_log().to_vec()).unwrap_or_default()
    }
}

/// A title that names no document: what a web browser writes when it saves a blank page or a
/// pop-up as PDF (`about:blank`), another browser-internal address, or a bare "Untitled". Such a
/// title says less than the file name, so the file name is shown instead. An address is one
/// word: "About: our company" is a real title.
pub(crate) fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    let address = !t.contains(char::is_whitespace) && ["about:", "blob:", "data:"].iter().any(|p| t.starts_with(p));
    address || t == "untitled"
}

/// The working file plus appearances for comments that have none, and for form fields the file
/// leaves to the viewer (#3470). Signature artwork is preserved; validation badges belong to the
/// viewer and never enter these bytes, which image/text exports and Print as Image also consume.
pub(crate) fn display_bytes(editor: Option<&mut Editor>, bytes: &Arc<Vec<u8>>) -> Arc<Vec<u8>> {
    let Some(e) = editor else { return bytes.clone() };
    let comments = pdfcraft_annot::with_missing_appearances_cached(&e.cos, &mut e.appearances);
    let fields = pdfcraft_forms::with_field_appearances_cached(comments.as_ref().unwrap_or(&e.cos), &mut e.field_appearances);
    let Some(display) = fields.or(comments) else {
        return bytes.clone();
    };
    match write_incremental(&display, &SaveOptions::default()) {
        Ok(written) => Arc::new(written),
        Err(err) => {
            e.cos.note_repair(format!("missing comment or field appearances could not be drawn ({err})"));
            bytes.clone()
        }
    }
}

/// `comments` (the display copy with comment appearances, if it needed any; else the document
/// itself) with its form fields drawn from their values where the file leaves that to the viewer.
fn with_field_appearances(comments: Option<pdfcraft_cos::Document>, doc: &pdfcraft_cos::Document) -> Option<pdfcraft_cos::Document> {
    pdfcraft_forms::with_field_appearances(comments.as_ref().unwrap_or(doc)).or(comments)
}

/// The reader shows the same pages in `a` and `b`: count, displayed size and rotation.
pub(crate) fn same_pages(a: &[pdfcraft_render::PageInfo], b: &[pdfcraft_render::PageInfo]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| (x.width - y.width).abs() < 0.01 && (x.height - y.height).abs() < 0.01 && x.rotation == y.rotation)
}

/// Each document-information field of `info` with its value in `cos` (`/Info`, else XMP), read
/// the way the full inspection reads them: NULs and spaces trimmed, empty is none.
pub(crate) fn info_fields<'a>(info: &'a mut DocInfo, cos: &pdfcraft_cos::Document) -> [(&'a mut Option<String>, Option<String>); 6] {
    let read = |key| pdfcraft_organize::info(cos, key).map(|v| v.trim_matches('\0').trim().to_string()).filter(|v| !v.is_empty());
    [
        (&mut info.title, read("Title")),
        (&mut info.author, read("Author")),
        (&mut info.subject, read("Subject")),
        (&mut info.keywords, read("Keywords")),
        (&mut info.creator, read("Creator")),
        (&mut info.producer, read("Producer")),
    ]
}

/// The copy PDFCraft displays when opening `bytes`: with appearances drawn, in memory only, for
/// comments that have none and form fields the file leaves to the viewer. Render paths outside the app, such as the CLI, use this so their page
/// images agree with the app and independent viewers. It never changes the source document, and an
/// unparseable file, a write failure or a parser panic falls back to the original bytes.
pub fn display_bytes_for(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Arc<Vec<u8>> {
    guard(|| {
        pdfcraft_cos::Document::open_with_password(bytes.clone(), password)
            .ok()
            .and_then(|cos| with_field_appearances(pdfcraft_annot::with_missing_appearances(&cos), &cos))
            .and_then(|display| write_incremental(&display, &SaveOptions::default()).ok())
            .map(Arc::new)
    })
    .ok()
    .flatten()
    .unwrap_or(bytes)
}

/// The password to give the renderer (`inspect`, `PageRenderer`) for `bytes` opened with
/// `password`. The renderer authenticates only the user password of R2–R4 files, so for their
/// owner password this is the user password that owner authentication recovers; otherwise it is
/// `password` itself. Render paths outside a [`Session`](crate::Session), such as the CLI, use this so the owner
/// password works there as it does in `doc_open`. A file that doesn't open with `password` keeps
/// it, so the renderer reports the same error it always did.
pub fn render_password_for(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Option<String> {
    let password = password?;
    let recovered =
        guard(|| pdfcraft_cos::Document::open_with_password(bytes, Some(password)).ok().and_then(|d| recovered_render_password(&d))).ok().flatten();
    Some(recovered.unwrap_or_else(|| password.to_owned()))
}

/// The user password owner authentication recovered from an R2–R4 `cos`, as the renderer takes it.
pub(crate) fn recovered_render_password(cos: &pdfcraft_cos::Document) -> Option<String> {
    cos.security().and_then(|s| s.recovered_user_password()).map(|user| renderer_password(&user))
}

/// Fails when the renderer still draws text under an applied redaction mark.
///
/// Redaction decides what lies under a mark from the positions its own interpreter computes, and
/// its verify pass reuses them. Where they differ from the renderer's (a standard font without
/// /Widths, whose advances the interpreter approximates; a CMap
/// that it reads differently), a mark drawn over the text as displayed lies over glyphs the
/// interpreter believes are elsewhere: they stay in the file and nothing notices. The renderer is
/// what the user saw when placing the marks, so it is asked. `after` is written as a save would
/// write it, without the marks' own `overlays` (the box and its label are drawn on purpose), its
/// text is extracted, and a glyph still mostly inside a mark fails the edit: the caller keeps
/// the document as it was.
pub(crate) fn shown_under_redactions(
    after: &pdfcraft_cos::Document,
    marks: &[pdfcraft_redact::Mark],
    overlays: &[pdfcraft_cos::ObjRef],
    pages: &[pdfcraft_render::PageInfo],
    password: Option<&str>,
) -> Result<(), EditError> {
    use pdfcraft_cos::Object;
    use pdfcraft_redact::RedactError;
    let mut marked: Vec<usize> = marks.iter().map(|m| m.page).collect();
    marked.sort_unstable();
    marked.dedup();
    if marked.is_empty() {
        return Ok(());
    }
    let mut shown = after.clone();
    let model = pdfcraft_model::pages(&shown);
    for &pi in &marked {
        let Some(page) = model.get(pi) else { continue };
        let Some(Object::Array(list)) = page.dict.get(b"Contents").map(|c| (*shown.resolve(c)).clone()) else { continue };
        let kept: Vec<Object> = list.into_iter().filter(|o| !o.as_ref().is_some_and(|r| overlays.contains(&r))).collect();
        shown.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept))).map_err(|e| EditError::Write(e.to_string()))?;
    }
    let bytes = write_incremental(&shown, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?;
    let config = RenderConfig { password: password.map(Arc::from), ..Default::default() };
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), config);
    let mut residue = 0;
    for &pi in &marked {
        let request = pdfcraft_render::RenderRequest { page: pi, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
        // A page that cannot be read back cannot be confirmed clean.
        let (Some(info), Some(text)) = (pages.get(pi), renderer.render(request).text) else { return Err(RedactError::Unreadable(pi + 1).into()) };
        let rects: Vec<[f64; 4]> = marks
            .iter()
            .filter(|m| m.page == pi)
            .flat_map(|m| m.rects.iter())
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])])
            .collect();
        for glyph in text.glyphs.iter().filter(|g| !g.text.trim().is_empty()) {
            let [x0, y0, x1, y1] = info.view_rect_to_user(glyph.rect).map(f64::from);
            let area = (x1 - x0) * (y1 - y0);
            if area.is_nan() || area <= 0.0 {
                continue;
            }
            // Redaction itself takes any glyph a fifth under a mark; one that is half under it
            // and still drawn was not where redaction thought it was.
            let under = |r: &[f64; 4]| (x1.min(r[2]) - x0.max(r[0])).max(0.0) * (y1.min(r[3]) - y0.max(r[1])).max(0.0) >= 0.5 * area;
            if rects.iter().any(under) {
                residue += 1;
            }
        }
    }
    if residue > 0 {
        return Err(RedactError::Residue(residue).into());
    }
    Ok(())
}
