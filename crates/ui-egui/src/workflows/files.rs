//! Multi-document page operations: Combine Files, Insert Pages from File, Extract Pages, Split.
//!
//! Files are picked and written through the host's `Files` service on every target: a pick is
//! asked for now and used on a later frame, once `process_picks` hands its answer back.

use std::sync::Arc;

use craft::docpath::names::{CopyNumbering as _, is_plain_file_name, safe_file_stem};
use craft::host::{PickRequest, Picked, WriteIntent};
use craft::ui::contract::OsFile;
use pdfcraft_engine::{DocId, Edit, SplitBy};

use crate::PdfCraftApp;

/// Why files were picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilePurpose {
    TextFont {
        page: usize,
    },
    Combine,
    InsertPages,
    ReplacePages,
    /// Scan & OCR ▸ Recognize text in multiple files.
    Ocr,
    /// Create a PDF ▸ Multiple files (PDFs, images and text).
    CreateMultiple,
    /// Create one PDF per selected non-PDF file.
    CreateMultipleSeparate,
    /// Find text and redact ▸ Import list: a text file with one word or phrase per line.
    RedactWords,
    /// Sign ▸ the Digital ID file to use or export.
    DigitalId,
    /// Sign ▸ the picture used as a signature's graphic.
    SignGraphic,
    /// Sign ▸ where the signed copy is saved.
    SignedCopy,
}

impl FilePurpose {
    /// The purpose string of a pick: picks of one purpose share a remembered folder.
    pub fn key(self) -> &'static str {
        match self {
            Self::TextFont { .. } => "text.font",
            Self::Combine => "combine",
            Self::InsertPages => "insert.pages",
            Self::ReplacePages => "replace.pages",
            Self::Ocr => "ocr",
            Self::CreateMultiple => "create.multiple",
            Self::CreateMultipleSeparate => "create.separate",
            Self::RedactWords => "redact.words",
            Self::DigitalId => "sign.digital_id",
            Self::SignGraphic => "sign.graphic",
            Self::SignedCopy => "sign.signed_copy",
        }
    }

    /// Insert and Replace edit the active document, at its selection.
    pub(crate) fn edits_active_document(self) -> bool {
        matches!(self, Self::InsertPages | Self::ReplacePages | Self::TextFont { .. })
    }
}

/// The fonts installed on this machine that documents may use, and the catalogue over them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct InstalledFonts {
    /// The scanned font files.
    pub files: Vec<pdfcraft_engine::text_fonts::FontFile>,
    /// A catalogue of exactly those files, built once when the scan arrives.
    pub catalog: Arc<craft::font_catalog::Catalog>,
}

/// The picker for `purpose`: PDFs, and for Create and Insert also what they convert.
fn picker_request(purpose: FilePurpose, multiple: bool) -> PickRequest {
    let request = if multiple { PickRequest::open_many(purpose.key()) } else { PickRequest::open(purpose.key()) };
    match purpose {
        FilePurpose::TextFont { .. } => request.filter("TrueType / OpenType font", &["ttf", "otf"]),
        FilePurpose::RedactWords => request.filter(tl!("Text"), &["txt"]),
        FilePurpose::CreateMultiple | FilePurpose::CreateMultipleSeparate | FilePurpose::InsertPages | FilePurpose::Combine => {
            let all: Vec<&str> = std::iter::once("pdf").chain(pdfcraft_engine::CONVERTIBLE).collect();
            request.filter(tl!("PDF, images and text"), &all).filter("PDF", &["pdf"])
        }
        _ => request.filter("PDF", &["pdf"]),
    }
}

/// The Replace Pages dialog: the chosen file and the ranges (1-based, inclusive).
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaceDraft {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub src_pages: usize,
    pub from: usize,
    pub to: usize,
    pub src_from: usize,
    /// The selection when the dialog opened (0-based). OK replaces these pages until the
    /// original range is edited, so a gap between selected pages is left alone.
    pub pages: Vec<usize>,
    /// The user changed the original from/to range, so OK uses that span.
    pub range_edited: bool,
    /// The document whose pages are replaced, as it was when the dialog opened: OK replaces
    /// nothing if another document is active by then (a file opened meanwhile), or it changed.
    pub(crate) target: Option<PickTarget>,
}

/// The document a pick edits, as it was when the picker opened. Page positions captured then
/// are only valid while it is unchanged, and the pick acts on the active document; so a pick
/// that arrives after the document was edited, or stopped being the active one, is refused
/// (see [`PdfCraftApp::still_pick_target`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickTarget {
    doc: DocId,
    generation: Option<u64>,
}

/// What Split does once confirmed.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitPlan {
    By(SplitBy),
    Size(usize),
    Bookmarks,
}

/// Organize ▸ Extract options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtractOptions {
    /// Each page as its own file.
    pub separate: bool,
    /// Delete the pages after extracting them.
    pub delete: bool,
    /// The name the extracted pages are saved under, without `.pdf` (#737). The dialog fills in
    /// the document's name when it opens; empty means that name too. As separate files each
    /// page is `<name> (page N).pdf`, and a single page the user renamed is `<name>.pdf`.
    pub name: String,
    /// The pages to extract as typed ranges, 1-based ("1-5, 8, 11-13") (#3593). Empty means the
    /// selected pages (or the current one); the dialog opens with it empty.
    pub pages: String,
}

/// The pages to extract (0-based, in the order typed, each once) from a document of `count`
/// pages: the typed ranges in `text`, or `selected` when none are typed. Says what is wrong with
/// a range that can't be read or takes no page.
pub(crate) fn pages_to_extract(text: &str, selected: Vec<usize>, count: usize) -> Result<Vec<usize>, String> {
    if text.trim().is_empty() {
        return Ok(selected);
    }
    match pdfcraft_engine::print::select_pages(count, Some(text), &[], pdfcraft_engine::print::Subset::All, false) {
        Ok(p) if p.is_empty() => Err(tl!("No pages selected").to_string()),
        Ok(p) => {
            let mut seen = std::collections::HashSet::new();
            Ok(p.into_iter().filter(|page| seen.insert(*page)).collect())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// A file name's last component ("file.pdf" when the path has none).
fn name_of(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file.pdf".into())
}

/// A picked file's name and bytes, read through the host's file service as every pick is, or its
/// name and why it can't be read.
#[cfg(not(target_arch = "wasm32"))]
fn read_named(host: &craft::host::Host, path: &std::path::Path) -> Result<(String, Vec<u8>), (String, String)> {
    let name = name_of(path);
    match host.files().read(path) {
        Ok(bytes) => Ok((name, bytes)),
        Err(e) => Err((name, e.to_string())),
    }
}

/// The name of a file dropped on the window ("dropped.pdf" when it has none).
pub(crate) fn dropped_name(f: &OsFile) -> String {
    if f.name.is_empty() { "dropped.pdf".into() } else { f.name.clone() }
}

impl PdfCraftApp {
    /// Ask for files to add to the Combine files list (its Add files… button).
    pub fn combine_dialog(&mut self) {
        self.pick_files(FilePurpose::Combine, true);
    }

    /// Create a PDF ▸ Multiple files: ask for the files to convert.
    pub fn create_multiple_dialog(&mut self) {
        self.pick_files(FilePurpose::CreateMultiple, true);
    }

    /// Create a PDF ▸ Separate PDFs: pick source files, then a destination folder.
    pub fn create_multiple_separate_dialog(&mut self) {
        self.pick_files(FilePurpose::CreateMultipleSeparate, true);
    }

    /// Scan & OCR ▸ Recognize text ▸ In multiple files: ask for the PDFs.
    pub fn ocr_files_dialog(&mut self) {
        self.pick_files(FilePurpose::Ocr, true);
    }

    /// Ask for files whose pages to insert after the selection (Organize ▸ Insert from file).
    pub fn insert_from_file_dialog(&mut self) {
        self.insert_from_file_at(None);
    }

    /// Ask for files to insert at grid gap `at` (0 = before the first page), or after the
    /// selection.
    pub(crate) fn insert_from_file_at(&mut self, at: Option<usize>) {
        let Some(i) = self.active else {
            self.notify_tr("Open a document first");
            return;
        };
        if let Some(v) = self.views.get_mut(i) {
            v.insert_at = at;
        }
        self.pick_files(FilePurpose::InsertPages, true);
    }

    /// Ask for files for `purpose`; they are used when the pick is answered.
    pub(crate) fn pick_files(&mut self, purpose: FilePurpose, multiple: bool) {
        // The document to edit is the one active now, not whichever is active when the pick is
        // answered (#167).
        let target = purpose.edits_active_document().then(|| self.pick_target(self.active_ids().map(|(_, id)| id))).flatten();
        self.pick_file(picker_request(purpose, multiple), target, move |app, ctx, picked| app.use_picked(ctx, purpose, picked));
    }

    /// Use what a pick for `purpose` answered. A cancelled or unavailable picker changes nothing
    /// (`process_picks` reports it).
    pub(crate) fn use_picked(&mut self, ctx: &egui::Context, purpose: FilePurpose, picked: Picked) {
        match picked {
            #[cfg(not(target_arch = "wasm32"))]
            Picked::Paths(paths) => self.use_paths(ctx, purpose, &paths),
            Picked::Contents(files) if !files.is_empty() => self.use_files(ctx, purpose, files),
            _ => {}
        }
    }

    /// Read the picked files and use them.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn use_paths(&mut self, ctx: &egui::Context, purpose: FilePurpose, paths: &[std::path::PathBuf]) {
        if matches!(purpose, FilePurpose::DigitalId | FilePurpose::SignGraphic | FilePurpose::SignedCopy) {
            return;
        }
        if purpose == FilePurpose::CreateMultiple {
            // Each file is read when its turn comes, so the selection's bytes are not all held at once.
            if !paths.is_empty() {
                let host = self.host.clone();
                self.stage_create_multiple(paths.iter().map(|p| read_named(&host, p)));
            }
            return;
        }
        if purpose == FilePurpose::CreateMultipleSeparate {
            if !paths.is_empty() {
                let host = self.host.clone();
                self.ask_separate_folder(paths.to_vec().into_iter().map(move |p| read_named(&host, &p)));
            }
            return;
        }
        let mut files = Vec::new();
        let mut modified = Vec::new();
        for p in paths {
            let name = name_of(p);
            if purpose == FilePurpose::RedactWords && std::fs::metadata(p).is_ok_and(|m| m.len() > crate::redact_ui::MAX_WORD_LIST_BYTES as u64) {
                self.notify_fmt("{name} is too large for a word list (at most 1 MB).", &[("name", &name)]);
                return;
            }
            let read = if matches!(purpose, FilePurpose::TextFont { .. }) {
                use std::io::Read;
                std::fs::File::open(p).and_then(|f| {
                    let mut bytes = Vec::new();
                    f.take((16 << 20) + 1).read_to_end(&mut bytes)?;
                    Ok(bytes)
                })
            } else {
                std::fs::read(p)
            };
            match read {
                Ok(b) => files.push((name, b)),
                Err(e) => {
                    self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return;
                }
            }
            modified.push(std::fs::metadata(p).and_then(|m| m.modified()).ok());
        }
        if files.is_empty() {
            return;
        }
        // Combine files lists when each file was last modified.
        if purpose == FilePurpose::Combine {
            let incoming = files
                .into_iter()
                .zip(modified)
                .map(|((name, bytes), modified)| crate::combine_ui::Incoming { name, bytes: Arc::new(bytes), modified, note: None, open_doc: None })
                .collect();
            self.stage_combine_with(incoming);
        } else {
            self.use_files(ctx, purpose, files);
        }
    }

    /// Ask for the folder the separate PDFs go to, then convert `sources` into it. Where there
    /// is no folder to choose (the web) they are downloaded at once.
    pub(crate) fn ask_separate_folder<I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = Result<(String, Vec<u8>), (String, String)>> + 'static,
    {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_file(PickRequest::folder("export.separate").title(tl!("Choose a folder for the exported files")), None, move |app, _, picked| {
            if let Some(dir) = picked.first_path() {
                app.stage_create_separate(sources, Some(dir));
            }
        });
        #[cfg(target_arch = "wasm32")]
        self.stage_create_separate(sources, None);
    }

    /// Files dropped on the Combine files tab join its list in the order dropped (see
    /// [`Self::add_dropped_to_combine`]); a folder adds the files in it and its subfolders.
    pub(crate) fn drop_into_combine(&mut self, ctx: &egui::Context, files: Vec<OsFile>) {
        let mut incoming = Vec::new();
        for f in files {
            let name = dropped_name(&f);
            // A file on disk is read from there, with when it was last modified.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(path) = f.path.as_deref().filter(|p| p.is_absolute()) {
                if path.is_dir() {
                    // After the files dropped before it.
                    self.add_dropped_to_combine(std::mem::take(&mut incoming));
                    self.add_folder_to_combine(ctx, path, true);
                    continue;
                }
                let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                match std::fs::read(path) {
                    Ok(bytes) => incoming.push(crate::combine_ui::Incoming { modified, ..crate::combine_ui::Incoming::new(name, bytes) }),
                    Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
                }
                continue;
            }
            // A drop that carries only bytes (no path) is taken as it is.
            match &f.bytes {
                Some(bytes) => incoming.push(crate::combine_ui::Incoming::new(name, bytes.to_vec())),
                None => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", tl!("the file has no content"))]),
            }
        }
        self.add_dropped_to_combine(incoming);
    }

    /// `doc` as it is now, for a pick that will edit it.
    pub(crate) fn pick_target(&self, doc: Option<DocId>) -> Option<PickTarget> {
        doc.map(|doc| PickTarget { doc, generation: self.session.get(doc).map(|d| d.edit_generation()) })
    }

    /// Whether a pick for `target` may still be used: the document is still the active one and
    /// unedited. Tells the user when not.
    pub(crate) fn still_pick_target(&mut self, target: Option<PickTarget>) -> bool {
        let Some(t) = target else { return true };
        let active = self.active_ids().map(|(_, id)| id);
        let generation = self.session.get(t.doc).map(|d| d.edit_generation());
        if active == Some(t.doc) && generation == t.generation {
            return true;
        }
        self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
        false
    }

    /// Use picked files (also the entry point for tests and automation).
    pub fn use_files(&mut self, ctx: &egui::Context, purpose: FilePurpose, files: Vec<(String, Vec<u8>)>) {
        match purpose {
            FilePurpose::TextFont { page } => {
                let Some((_, bytes)) = files.into_iter().next() else { return };
                let bytes = Arc::new(bytes);
                if !self.apply_edit(ctx, Edit::RegisterTextFont { page, bytes: bytes.clone() }) {
                    return;
                }
                let Some((index, id)) = self.active_ids() else { return };
                let Some(mut ed) = self.views.get(index).and_then(|v| v.session.line_editor.clone()).filter(|ed| ed.page == page) else { return };
                // The font just loaded is the one the user wants: apply it to the paragraph, as
                // choosing it from the document-font menu does, instead of only registering it (#2591).
                let resource = self.session.get(id).and_then(|doc| {
                    doc.text_editor_fonts(page).into_iter().find(|f| f.bytes == *bytes && !f.resource.starts_with("PCApproved")).map(|f| f.resource)
                });
                if resource.is_some() {
                    ed.font_resource = resource;
                    self.apply_line_format(ctx, index, ed);
                }
            }
            FilePurpose::Combine => self.stage_combine(files),
            FilePurpose::InsertPages => self.insert_files(ctx, files),
            FilePurpose::ReplacePages => {
                if let Some((name, bytes)) = files.into_iter().next() {
                    self.start_replace(ctx, name, bytes);
                }
            }
            FilePurpose::Ocr => self.ocr_files(ctx, files),
            FilePurpose::CreateMultiple => self.stage_create_multiple(files.into_iter().map(Ok)),
            FilePurpose::CreateMultipleSeparate => self.ask_separate_folder(files.into_iter().map(Ok)),
            FilePurpose::RedactWords => {
                let Some((name, bytes)) = files.into_iter().next() else { return };
                if bytes.len() > crate::redact_ui::MAX_WORD_LIST_BYTES {
                    self.notify_fmt("{name} is too large for a word list (at most 1 MB).", &[("name", &name)]);
                    return;
                }
                self.redact_search.words = pdfcraft_engine::redact_word_list(&pdfcraft_engine::decode_text(&bytes)).join("\n");
                self.redact_search.mode = crate::redact_ui::SearchMode::Words;
            }
            // Picked by the signing tools with their own requests.
            FilePurpose::DigitalId | FilePurpose::SignGraphic | FilePurpose::SignedCopy => {}
        }
    }

    pub fn replace_pages_dialog(&mut self) {
        if self.active.is_none() {
            self.notify_tr("Open a document first");
            return;
        }
        self.pick_files(FilePurpose::ReplacePages, false);
    }

    /// Insert the pages of `files` (PDFs, images, text), in order, at the gap a "+" in the page
    /// grid chose, or else after the selection (or the current page); then select them.
    pub fn insert_files(&mut self, ctx: &egui::Context, files: Vec<(String, Vec<u8>)>) {
        self.insert_each(ctx, files.into_iter().map(Ok));
    }

    /// [`Self::insert_files`] for files read as their turn comes: one that can't be read is skipped
    /// and named.
    pub(crate) fn insert_each<I>(&mut self, ctx: &egui::Context, files: I)
    where
        I: IntoIterator<Item = Result<(String, Vec<u8>), (String, String)>>,
    {
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        let (id, chosen) = (view.id, view.insert_at.take());
        let after = view.target_pages().last().map_or(0, |p| p + 1);
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let start = chosen.unwrap_or(after).min(count);
        let mut at = start;
        for file in files {
            let (name, bytes) = match file {
                Ok(file) => file,
                Err((name, error)) => {
                    self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &error)]);
                    continue;
                }
            };
            match self.session.insert_source_inline(id, &name, &Arc::new(bytes)) {
                Ok((pages, bytes)) => {
                    if self.apply_edit(
                        ctx,
                        Edit::InsertPagesFrom { name, bytes, pages: None, at, field_conflicts: pdfcraft_engine::FieldConflicts::Rename },
                    ) {
                        at = at.saturating_add(pages);
                    }
                }
                Err(e) => self.notify_error(e),
            }
        }
        if at > start
            && let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
        {
            view.select_pages(&(start..at).collect::<Vec<_>>());
        }
    }

    /// Copy the pages typed in Extract pages, else the selected pages (or the current page), into
    /// a new unsaved document tab.
    pub fn extract_selection(&mut self, ctx: &egui::Context, opts: ExtractOptions) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((i, id)) = self.active_ids() else { return };
        let pages = match pages_to_extract(&opts.pages, self.views[i].target_pages(), self.views[i].page_count()) {
            Ok(pages) => pages,
            Err(e) => {
                self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e)]);
                return;
            }
        };
        // Deleting takes the pages in document order (typed ranges may run backwards).
        let mut doomed = pages.clone();
        doomed.sort_unstable();
        let stem = self.session.get(id).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_default();
        // The name typed in the dialog (#737), else the document's.
        let chosen = safe_file_stem(&opts.name, "pdf");
        let renamed = !chosen.is_empty() && chosen != safe_file_stem(&stem, "pdf");
        let base = if chosen.is_empty() { stem.clone() } else { chosen };
        if opts.separate {
            // Each page as its own file, in a chosen folder.
            let mut named = Vec::new();
            for &p in &pages {
                match self.session.extract(id, &[p]) {
                    // One page the user named is saved under exactly that name.
                    Ok(bytes) if renamed && pages.len() == 1 => named.push((format!("{base}.pdf"), bytes)),
                    Ok(bytes) => named.push((format!("{base} (page {}).pdf", p + 1), bytes)),
                    Err(e) => {
                        self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e.to_string())]);
                        return;
                    }
                }
            }
            // Delete the pages only once their files are written, which is on a later frame when
            // the user chooses the folder, and only if they are still what was written: the
            // document must be unedited since (page numbers shift, and edits to those pages
            // aren't in the files) and still the active one (deleting edits the active document).
            let generation = self.session.get(id).map(|d| d.edit_generation());
            let delete = move |app: &mut Self, ctx: &egui::Context| {
                if !opts.delete {
                    return;
                }
                let unchanged = app.session.get(id).map(|d| d.edit_generation()) == generation;
                if unchanged && app.active_ids().map(|(_, active)| active) == Some(id) {
                    app.apply_edit(ctx, pdfcraft_engine::Edit::DeletePages { pages: doomed });
                } else {
                    app.notify_tr("The pages were extracted but not deleted, because the document changed while you were choosing a folder.");
                }
            };
            #[cfg(not(target_arch = "wasm32"))]
            self.write_files_then(named, tl!("Choose a folder for the extracted pages"), delete);
            // The web has no folder to ask for: the files download now, and the pages go with them.
            #[cfg(target_arch = "wasm32")]
            if self.download_files(&named) {
                delete(self, ctx);
            }
            return;
        }
        match self.session.extract(id, &pages) {
            Ok(bytes) => {
                let message = if pages.len() == 1 {
                    tl!("Extracted 1 page").to_string()
                } else {
                    crate::i18n::fmt(tl!("Extracted {n} pages"), &[("n", &pages.len().to_string())])
                };
                // Save As offers the tab's name, so a name typed in the dialog is kept.
                let name = if renamed { format!("{base}.pdf") } else { format!("{stem} (extract).pdf") };
                self.open_created(&name, bytes, &message)
            }
            Err(e) => {
                self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e.to_string())]);
                return;
            }
        }
        if opts.delete {
            // Back on the original document.
            self.active = Some(i);
            self.apply_edit(ctx, pdfcraft_engine::Edit::DeletePages { pages: doomed });
        }
    }

    /// Write named files into a chosen folder (desktop) or as downloads (web).
    pub(crate) fn write_files(&mut self, named: &[(String, Arc<Vec<u8>>)], title: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        self.write_files_then(named.to_vec(), title, |_, _| {});
        #[cfg(target_arch = "wasm32")]
        {
            let _ = title;
            self.download_files(named);
        }
    }

    /// [`Self::write_files`], then `after` once every file is written, on the later frame when
    /// the user chooses the folder. `after` never runs when writing fails or is cancelled.
    /// Every name must be a plain file name ([`is_plain_file_name`]); otherwise nothing is written
    /// and no folder is asked for.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn write_files_then(
        &mut self,
        named: Vec<(String, Arc<Vec<u8>>)>,
        title: &str,
        after: impl FnOnce(&mut Self, &egui::Context) + 'static,
    ) {
        if !self.plain_file_names(&named) {
            return;
        }
        {
            let write = move |app: &mut Self, ctx: &egui::Context, picked: Picked| {
                let Some(dir) = picked.first_path() else { return };
                for (name, bytes) in &named {
                    if let Err(e) = app.host.files().write(&dir.join(name), bytes, WriteIntent::SaveAs) {
                        app.notify_fmt("Couldn't write {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                        return;
                    }
                }
                if named.len() == 1 {
                    app.notify_fmt("Wrote 1 file to {dir}", &[("dir", &dir.display().to_string())]);
                } else {
                    app.notify_fmt("Wrote {n} files to {dir}", &[("n", &named.len().to_string()), ("dir", &dir.display().to_string())]);
                }
                after(app, ctx);
            };
            self.pick_file(PickRequest::folder("export.folder").title(title), None, write);
        }
    }

    /// The web's [`Self::write_files`]: there is no folder to ask for, so each file is a download.
    /// Returns `true` once every file is handed over.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn download_files(&mut self, named: &[(String, Arc<Vec<u8>>)]) -> bool {
        if !self.plain_file_names(named) {
            return false;
        }
        for (name, bytes) in named {
            if let Err(e) = self.host.files().write(std::path::Path::new(name), bytes, WriteIntent::SaveAs) {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                return false;
            }
        }
        true
    }

    /// Whether every name in `named` is a plain file name ([`is_plain_file_name`]); otherwise the
    /// user is told which one is not.
    fn plain_file_names(&mut self, named: &[(String, Arc<Vec<u8>>)]) -> bool {
        let Some((name, _)) = named.iter().find(|(name, _)| !is_plain_file_name(name)) else { return true };
        self.notify_fmt("Couldn't write {name}: {e}", &[("name", name), ("e", tl!("the name is not a plain file name"))]);
        false
    }

    /// Pages ▸ Rotate Pages: rotate the pages of `base` that pass the parity and orientation
    /// filters.
    pub fn rotate_pages(
        &mut self,
        ctx: &egui::Context,
        degrees: i64,
        base: Vec<usize>,
        parity: pdfcraft_engine::PageParity,
        orientation: pdfcraft_engine::PageOrientation,
    ) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let pages = pdfcraft_engine::filter_pages(&doc.info, &base, parity, orientation);
        if pages.is_empty() {
            self.notify_tr("No pages match those choices");
            return;
        }
        self.apply_edit(ctx, pdfcraft_engine::Edit::RotatePages { pages, degrees });
    }

    /// Split the active document and write the parts: into a chosen folder (desktop, written on
    /// a later frame once the user has chosen it) or as downloads (web).
    pub fn split_active(&mut self, ctx: &egui::Context, plan: &SplitPlan) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let stem = self.session.get(id).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_else(|| "document".into());
        let (parts, titles) = match plan {
            SplitPlan::By(by) => (self.session.split(id, by), Vec::new()),
            SplitPlan::Size(max) => (self.session.split_by_size(id, *max), Vec::new()),
            SplitPlan::Bookmarks => {
                let marks = self.session.bookmark_splits(id);
                let cuts: Vec<usize> = marks.iter().map(|m| m.0).collect();
                (self.session.split(id, &SplitBy::Before(cuts)), marks)
            }
        };
        let parts = match parts {
            Ok(p) => p,
            Err(e) => {
                self.notify_fmt("Couldn't split: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let safe = |t: &str| t.chars().map(|c| if c.is_alphanumeric() || " -_.,()".contains(c) { c } else { '_' }).collect::<String>();
        let mut used = std::collections::HashSet::new();
        let named: Vec<(String, Arc<Vec<u8>>)> = parts
            .into_iter()
            .map(|(a, b, bytes)| {
                // Bookmark splits are named after the bookmark that starts the part.
                let title = titles.iter().find(|(p, _)| *p + 1 == a).map(|(_, t)| safe(t));
                let name = match title {
                    Some(t) => format!("{stem} - {t}.pdf"),
                    None if a == b => format!("{stem} (page {a}).pdf"),
                    None => format!("{stem} (pages {a}-{b}).pdf"),
                };
                // Equal titles (or titles that sanitize alike) get (2), (3), ... so no part overwrites another; compared case-insensitively.
                let mut unique = name.clone();
                let mut n = 1;
                while !used.insert(unique.to_lowercase()) {
                    n += 1;
                    unique = format!("{} ({n}).pdf", name.trim_end_matches(".pdf"));
                }
                (unique, bytes)
            })
            .collect();
        self.write_files(&named, "Choose a folder for the split files");
    }

    /// Summarize Comments: make the summary and open it as a new document.
    pub fn summarize_comments(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let stem =
            self.session.get(id).map(|d| std::path::Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        match self.session.summarize_comments(id, self.summary_sort) {
            Ok(bytes) => self.open_created(&format!("Summary of comments on {}.pdf", stem.unwrap_or_default()), bytes, "Created a comment summary"),
            Err(e) => self.notify_fmt("Couldn't summarize comments: {e}", &[("e", &e.to_string())]),
        }
    }

    /// Open `bytes` as a new unsaved document named `name` and say `message`.
    pub(crate) fn open_created(&mut self, name: &str, bytes: Arc<Vec<u8>>, message: &str) {
        match self.open_new_document(name, bytes) {
            Ok(()) => self.notify_tr(message),
            Err(e) => self.notify_fmt("Couldn't open the result: {e}", &[("e", &e)]),
        }
    }

    /// The fonts installed on this machine, scanned off the UI thread: `None` while the scan
    /// runs (and it is started if none is). `reload` rescans to find newly installed fonts or to
    /// retry a failed scan.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn installed_document_fonts(&mut self, reload: bool) -> Option<&InstalledFonts> {
        use pdfcraft_engine::text_fonts;
        if let Some(done) = self.font_scan.try_recv() {
            let catalog = Arc::new(craft::font_catalog::Catalog::of_files(done.result.iter().map(|f| f.path.clone()).collect()));
            self.document_fonts = Some(InstalledFonts { files: done.result, catalog });
        }
        if reload {
            self.document_fonts = None;
        }
        if self.document_fonts.is_none() && !self.font_scan.contains(&()) {
            self.submit_scan(|| text_fonts::discover(&text_fonts::installed_directories()));
        }
        self.document_fonts.as_ref()
    }

    /// Run `scan` as the font scan job. A scan that panics ends as an empty list: the queue
    /// delivers nothing for a panicked job, which would leave the menu waiting for ever.
    #[cfg(not(target_arch = "wasm32"))]
    fn submit_scan(&mut self, scan: impl FnOnce() -> Vec<pdfcraft_engine::text_fonts::FontFile> + Send + 'static) {
        use craft::host::queue::Priority;
        self.font_scan.submit((), Priority::BACKGROUND, move |_| pdfcraft_engine::guard(scan).unwrap_or_default());
    }
}

pub(crate) fn strip_pdf(name: &str) -> &str {
    name.strip_suffix(".pdf").or_else(|| name.strip_suffix(".PDF")).unwrap_or(name)
}

/// The folder holding the document at `path`, where a save picker starts the first time.
fn document_folder(path: Option<&str>) -> Option<std::path::PathBuf> {
    path.and_then(|p| std::path::Path::new(p).parent()).map(std::path::Path::to_path_buf)
}

impl PdfCraftApp {
    /// The files a pick answered, read: paths through the host's file service, browser contents
    /// as they are. A file that can't be read gives its name and why.
    pub(crate) fn read_picked(&self, picked: Picked) -> Result<Vec<(String, Vec<u8>)>, (String, String)> {
        match picked {
            Picked::Paths(paths) => paths
                .iter()
                .map(|p| {
                    let name = name_of(p);
                    self.host.files().read(p).map(|bytes| (name.clone(), bytes)).map_err(|e| (name, e.to_string()))
                })
                .collect(),
            Picked::Contents(files) => Ok(files),
            _ => Ok(Vec::new()),
        }
    }

    /// Prepare a form ▸ Import data: XFDF, FDF, XML, CSV or text.
    pub fn import_data_dialog(&mut self) {
        self.import_dialog(false);
    }

    /// Comments ▸ Import comments: the files Import data takes, or another PDF to copy the
    /// comments of.
    pub fn import_comments_dialog(&mut self) {
        self.import_dialog(true);
    }

    fn import_dialog(&mut self, pdf: bool) {
        let mut extensions = vec!["xfdf", "fdf", "xml", "xdp", "csv", "txt"];
        if pdf {
            extensions.insert(2, "pdf");
        }
        let request = PickRequest::open("import.data").title(tl!("Import data")).filter(tl!("Comment and form data"), &extensions);
        let target = self.pick_target(self.active_ids().map(|(_, id)| id));
        self.pick_file(request, target, |app, ctx, picked| match app.read_picked(picked) {
            Ok(files) => {
                for (name, bytes) in files {
                    app.apply_edit(ctx, pdfcraft_engine::Edit::ImportData { name, bytes: Arc::new(bytes) });
                }
            }
            Err((name, e)) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
        });
    }

    /// Export a PDF ▸ Word, HTML or RTF: ask where, then write.
    pub fn export_office_dialog(&mut self, ctx: &egui::Context, format: pdfcraft_engine::compare::OfficeFormat) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let ext = format.extension();
        let folder = document_folder(doc.path.as_deref());
        let bytes = doc.export_office(format);
        let mut request =
            PickRequest::save("export.office").title(tl!("Export")).filter(ext.to_uppercase(), &[ext]).file_name(format!("{stem}.{ext}"));
        if let Some(folder) = folder {
            request = request.default_directory(folder);
        }
        self.pick_file(request, None, move |app, _, picked| {
            let Some(path) = picked.first_path() else { return };
            match app.host.files().write(&path, &bytes, WriteIntent::SaveAs) {
                Ok(_) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            }
        });
    }

    /// Forms ▸ Merge data files into spreadsheet: choose data files (FDF, XFDF or filled-in PDF
    /// forms), then where to save the CSV.
    pub fn merge_data_dialog(&mut self) {
        let request = PickRequest::open_many("merge.data")
            .title(tl!("Select data files to merge"))
            .filter(tl!("Form data and PDF forms"), &["fdf", "xfdf", "pdf"]);
        self.pick_file(request, None, |app, _, picked| match app.read_picked(picked) {
            // Asks where to save: a second picker, now that the first one has closed.
            Ok(files) if !files.is_empty() => app.merge_data_files(files),
            Ok(_) => {}
            Err((name, e)) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
        });
    }

    /// Merge the given data files and save the spreadsheet (asks where).
    pub fn merge_data_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        let csv = match pdfcraft_engine::merge_data_files(&files) {
            Ok(c) => c,
            Err(e) => return self.notify_error(e),
        };
        let count = files.len();
        let request = PickRequest::save("export.data").title(tl!("Save the spreadsheet")).filter("CSV", &["csv"]).file_name("report.csv");
        self.pick_file(request, None, move |app, _, picked| {
            let Some(path) = picked.first_path() else { return };
            match app.host.files().write(&path, csv.as_bytes(), WriteIntent::SaveAs) {
                Ok(_) if count == 1 => app.notify_fmt("Merged 1 file into {path}", &[("path", &path.display().to_string())]),
                Ok(_) => app.notify_fmt("Merged {n} files into {path}", &[("n", &count.to_string()), ("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            }
        });
    }

    /// Export all comments / form data: the format follows the file name's extension.
    pub fn export_data_dialog(&mut self, ctx: &egui::Context, comments: bool, fields: bool) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let folder = document_folder(doc.path.as_deref());
        let xfa = doc.info.xfa.is_some();
        let title = if comments { tl!("Export comments") } else { tl!("Export form data") };
        let mut request = PickRequest::save("export.data").title(title).file_name(format!("{stem}.xfdf"));
        request = if comments {
            request.filter("XFDF", &["xfdf"]).filter("FDF", &["fdf"])
        } else if xfa {
            // An XFA form's data, as Acrobat exports it, first.
            request
                .file_name(format!("{stem}.xml"))
                .filter("XML", &["xml"])
                .filter("XDP", &["xdp"])
                .filter("XFDF", &["xfdf"])
                .filter("FDF", &["fdf"])
                .filter("CSV", &["csv"])
                .filter(tl!("Text"), &["txt"])
        } else {
            request.filter("XFDF", &["xfdf"]).filter("FDF", &["fdf"]).filter("XML", &["xml"]).filter("CSV", &["csv"]).filter(tl!("Text"), &["txt"])
        };
        if let Some(folder) = folder {
            request = request.default_directory(folder);
        }
        self.pick_file(request, None, move |app, ctx, picked| {
            let Some(path) = picked.first_path() else { return };
            // Include what was typed while the save panel was open (#166).
            if !app.commit_typing_in(ctx, id) {
                return;
            }
            let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let format = pdfcraft_engine::DataFormat::from_extension(&ext).unwrap_or(pdfcraft_engine::DataFormat::Xfdf);
            match app.session.export_data(id, format, comments, fields) {
                Ok(bytes) => match app.host.files().write(&path, &bytes, WriteIntent::SaveAs) {
                    Ok(_) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                    Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
                },
                Err(e) => app.notify_error(e),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles};

    use crate::PdfCraftApp;

    fn app_with(files: Arc<ScriptedFiles>, jobs: Arc<ManualJobs>) -> PdfCraftApp {
        let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).files(files).jobs(jobs).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app
    }

    #[test]
    fn write_files_refuses_names_that_leave_the_folder() {
        let out = PathBuf::from("/scripted/out");
        let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out.clone()])]));
        let mut app = app_with(files.clone(), Arc::new(ManualJobs::new()));
        let absolute = "/scripted/abs.pdf";
        let pdf = || Arc::new(b"%PDF".to_vec());
        for bad in ["../x.pdf", "..", ".", "", "sub/x.pdf", "sub\\x.pdf", "C:x.pdf", "x.pdf.", absolute] {
            // A plain name first: nothing is written when any name in the batch is refused.
            app.write_files(&[("ok.pdf".into(), pdf()), (bad.into(), pdf())], "t");
            // A fresh context each time: it holds only the notice this attempt posted.
            let said = crate::notices::texts(&mut app, &egui::Context::default());
            assert!(said.iter().any(|t| t.contains("not a plain file name")), "{bad}");
        }
        assert!(files.requests().is_empty(), "no folder is asked for");
        assert!(files.file(&out.join("ok.pdf")).is_none());
        app.write_files(&[("x.pdf".into(), pdf())], "t");
        app.process_picks(&egui::Context::default());
        assert_eq!(files.requests().len(), 1);
        assert_eq!(files.file(&out.join("x.pdf")).as_deref(), Some(&b"%PDF"[..]));
    }

    fn font(name: &str) -> pdfcraft_engine::text_fonts::FontFile {
        pdfcraft_engine::text_fonts::FontFile { path: format!("{name}.ttf").into(), family: name.into(), face: name.into() }
    }

    #[test]
    fn installed_fonts_are_listed_once_and_a_reload_lists_them_again() {
        let jobs = Arc::new(ManualJobs::new());
        let mut app = app_with(Arc::new(ScriptedFiles::new()), jobs.clone());
        assert!(app.installed_document_fonts(false).is_none());
        assert_eq!(jobs.waiting(), 1, "the first call starts one scan");
        assert!(app.installed_document_fonts(false).is_none());
        assert_eq!(jobs.waiting(), 1, "a second call while it waits starts no other");
        jobs.run_all();
        // The scan this test controls replaces the real one's result.
        app.submit_scan(|| vec![font("Test")]);
        jobs.run_all();
        let listed = app.installed_document_fonts(false);
        assert!(listed.is_some());
        let _ = listed;
        assert!(app.installed_document_fonts(true).is_none(), "a reload clears the list");
        assert_eq!(jobs.waiting(), 1, "and starts one more scan");
    }

    #[test]
    fn a_panicking_font_scan_finishes_with_an_empty_list() {
        let jobs = Arc::new(ManualJobs::new());
        let mut app = app_with(Arc::new(ScriptedFiles::new()), jobs.clone());
        app.submit_scan(|| panic!("synthetic font scan failure"));
        jobs.run_all();
        let listed = app.installed_document_fonts(false).expect("the scan ended");
        assert!(listed.files.is_empty());
        assert_eq!(jobs.waiting(), 0, "no second scan is started");
    }
}
