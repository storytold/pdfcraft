use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::ui::choice::Checkbox;
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::ComboBox;
use craft::ui::sections::{DialogRole, FooterButton};
use craft_ui::buttons::Button;
use craft_ui::choice::Segmented;
use craft_ui::contract::{Bind, Entry, Props, Show};
use egui::{Align, Color32, Layout, RichText};
use pdfcraft_engine::print;

use crate::PdfCraftApp;
use crate::workflows::print_ui::{self, Handling, PrintDraft, Which};

#[derive(Clone, Debug)]
pub struct BatchPrintItem {
    pub name: String,
    pub path: Option<PathBuf>,
    pub doc_id: Option<pdfcraft_engine::DocId>,
    pub bytes: Option<Arc<Vec<u8>>>,
    pub page_count: usize,
    pub page_sizes: Vec<(f64, f64)>,
    pub page_labels: Vec<String>,
    pub selected: bool,
    pub is_tab: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum BatchPrintStep {
    #[default]
    SelectFiles,
    Settings,
}

#[derive(Default)]
pub struct BatchPrintDraft {
    pub open: bool,
    pub session_id: u64,
    pub step: BatchPrintStep,
    pub items: Vec<BatchPrintItem>,
    pub preview_cache: HashMap<usize, egui::TextureHandle>,
    pub last_grayscale: bool,
    pub preview_sheet: usize,
    pub settings_initialized: bool,
}

impl BatchPrintDraft {
    pub fn clear(&mut self) {
        self.step = BatchPrintStep::SelectFiles;
        self.items.clear();
        self.preview_cache.clear();
        self.preview_sheet = 0;
        self.settings_initialized = false;
    }

    pub fn select_all(&mut self, select: bool) {
        for item in &mut self.items {
            item.selected = select;
        }
    }

    pub fn select_only_open(&mut self) {
        for item in &mut self.items {
            item.selected = item.is_tab;
        }
    }

    pub fn add_folder_pdfs(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut pdf_files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                pdf_files.push(path);
            }
        }
        pdf_files.sort();

        for path in pdf_files {
            if self.items.iter().any(|i| i.path.as_deref() == Some(&path)) {
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let mut page_sizes = Vec::new();
            let mut page_labels = Vec::new();
            let mut file_arc = None;
            if let Ok(bytes) = std::fs::read(&path) {
                let arc_bytes = Arc::new(bytes);
                file_arc = Some(arc_bytes.clone());
                if let Ok(info) = pdfcraft_render::inspect(arc_bytes, None) {
                    for p in info.pages {
                        page_sizes.push((f64::from(p.width), f64::from(p.height)));
                        page_labels.push(p.label);
                    }
                }
            }
            if page_sizes.is_empty() {
                continue;
            }
            let page_count = page_sizes.len();
            self.items.push(BatchPrintItem {
                name,
                path: Some(path),
                doc_id: None,
                bytes: file_arc,
                page_count,
                page_sizes,
                page_labels,
                selected: true,
                is_tab: false,
            });
        }
    }
}

impl PdfCraftApp {
    /// Open the Batch Print dialog.
    pub fn open_batch_print(&mut self, ctx: &egui::Context) {
        if !self.commit_form_typing(ctx) {
            return;
        }
        self.batch_print.session_id = self.batch_print.session_id.wrapping_add(1);
        self.batch_print.clear();
        self.batch_print.open = true;
        self.batch_print_add_open_documents();
    }

    /// Add all open documents to batch print draft.
    pub fn batch_print_add_open_documents(&mut self) {
        for view in &self.views {
            if let Some(doc) = self.session.get(view.id)
                && !self.batch_print.items.iter().any(|i| i.doc_id == Some(doc.id))
            {
                let page_sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                let page_labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
                let page_count = doc.info.pages.len();
                self.batch_print.items.push(BatchPrintItem {
                    name: doc.name.clone(),
                    path: doc.path.as_ref().map(PathBuf::from),
                    doc_id: Some(doc.id),
                    bytes: Some(doc.bytes.clone()),
                    page_count,
                    page_sizes,
                    page_labels,
                    selected: true,
                    is_tab: true,
                });
            }
        }
    }

    /// Prepare print draft for batch print settings, reading printers in the background.
    pub fn prepare_batch_print_settings(&mut self) {
        let raw_printers = print_ui::listed_printers();
        self.record_listed(&raw_printers);
        let printers = print_ui::offered_printers(&raw_printers);

        if !self.batch_print.settings_initialized {
            let mut keep = std::mem::take(&mut self.print_draft);
            if keep.handling == Handling::Label {
                keep.set_handling(Handling::Size);
            }
            let default = print_ui::default_printer(&keep, &printers);
            let picked = keep.printer_picked && print_ui::offered(keep.printer.as_deref(), &printers);
            let printer = if picked { keep.printer.clone() } else { default };
            let changed = printer != keep.printer;
            let fresh = PrintDraft::default();
            self.print_draft = PrintDraft {
                copies: fresh.copies,
                range: fresh.range,
                subset: fresh.subset,
                reverse: fresh.reverse,
                handling: fresh.handling,
                per_sheet: fresh.per_sheet,
                custom_grid: fresh.custom_grid,
                cols: fresh.cols,
                rows: fresh.rows,
                order: fresh.order,
                border: fresh.border,
                auto_rotate: fresh.auto_rotate,
                booklet_subset: fresh.booklet_subset,
                poster_scale: fresh.poster_scale,
                overlap: fresh.overlap,
                cut_marks: fresh.cut_marks,
                label_flip: fresh.label_flip,
                orientation: fresh.orientation,
                crops: Vec::new(),
                crop_pages: None,
                printer_picked: picked,
                units: self.units,
                printer: printer.clone(),
                current_page: 0,
                sheet: 0,
                selected: Vec::new(),
                which: Which::All,
                area: None,
                custom_new: None,
                ..keep
            };
            if changed {
                self.print_draft.apply_setup();
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(p) = printer {
                self.read_papers_in_background(p);
            }
            self.batch_print.settings_initialized = true;
        } else {
            let current = self.print_draft.printer.clone();
            if !print_ui::offered(current.as_deref(), &printers) {
                let default = print_ui::default_printer(&self.print_draft, &printers);
                self.print_draft.printer = default.clone();
                self.print_draft.apply_setup();
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(p) = default {
                    self.read_papers_in_background(p);
                }
            }
        }
    }

    /// Ask for a folder of PDFs to include in batch printing.
    pub fn batch_print_pick_folder(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let title = tl!("Choose a folder of PDFs to print");
            let session_id = self.batch_print.session_id;
            self.pick_first(craft::host::PickRequest::folder("batch_print.folder").title(title), None, move |app, _, dir| {
                if app.batch_print.open && app.batch_print.session_id == session_id {
                    app.batch_print.add_folder_pdfs(&dir);
                }
            });
        }
    }

    /// Print each selected document with the current print settings.
    pub fn print_batch_now(&mut self, ctx: &egui::Context) -> bool {
        if !self.commit_form_typing(ctx) {
            return false;
        }

        let selected: Vec<(usize, BatchPrintItem)> =
            self.batch_print.items.iter().enumerate().filter(|(_, i)| i.selected).map(|(idx, i)| (idx, i.clone())).collect();

        if selected.is_empty() {
            return false;
        }

        let printer = self.print_draft.printer.clone();
        if printer.is_none() {
            self.notify_tr("Batch printing requires a physical or system printer (Save as PDF not supported for batch)");
            return false;
        }

        if self.print_draft.papers_loading() {
            self.notify_tr("Waiting for printer paper setup to finish");
            return false;
        }

        // Ensure printer's two-sided support is obtained/cached if unknown before building jobs
        if let Some(ref p) = printer
            && !self.print_draft.two_sided.contains_key(p)
            && let Some(ref source) = self.two_sided_source
        {
            let support = pdfcraft_engine::guard(|| source(p)).unwrap_or(None);
            self.print_draft.two_sided.insert(p.clone(), support);
        }

        let mut queued_count = 0usize;
        let mut failed_count = 0usize;

        for (idx, item) in &selected {
            let mut succeeded = false;

            if let Some(doc_id) = item.doc_id {
                if let Some(doc) = self.session.get(doc_id) {
                    let before = doc.edit_generation();
                    self.session.run_document_actions(doc_id, pdfcraft_engine::js::DocumentScriptEvent::WillPrint);
                    if self.session.get(doc_id).map(|d| d.edit_generation()) != Some(before) && self.print_draft.handling == Handling::Label {
                        self.print_draft.crop_pages = None;
                        self.read_label_crops(doc_id);
                    }
                }
                if self.print_draft.handling == Handling::Label {
                    self.print_draft.crop_pages = None;
                    self.read_label_crops(doc_id);
                }
                if let Some(doc) = self.session.get(doc_id) {
                    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
                    if let Ok(settings) = self.print_draft.settings(doc.info.pages.len(), &labels)
                        && let Ok(bytes) = self.session.print_pdf(doc_id, &settings)
                    {
                        let mut job = self.print_draft.job(&doc.name);
                        let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                        if let Ok(sheets) = print::layout(&sizes, &settings) {
                            print::add_cups_orientation(&mut job.options, &sheets);
                        }
                        if self.print_draft.handling == Handling::Booklet && self.print_draft.printer_two_sided().is_none() {
                            job.duplex = None;
                        }
                        if let Some(ref p) = printer
                            && self.send_to_printer(p.clone(), bytes, job, Some(*idx)).is_ok()
                        {
                            succeeded = true;
                        }
                    }
                }
            } else if let Some(path) = &item.path {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let file_bytes = match &item.bytes {
                        Some(b) => Some(b.clone()),
                        None => std::fs::read(path).ok().map(Arc::new),
                    };
                    if let Some(file_bytes) = file_bytes {
                        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if let Ok(temp_id) = self.session.open(&name, Some(path.to_string_lossy().to_string()), file_bytes, None) {
                            if self.print_draft.handling == Handling::Label {
                                self.print_draft.crop_pages = None;
                                self.read_label_crops(temp_id);
                            }
                            if let Some(doc) = self.session.get(temp_id) {
                                let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
                                if let Ok(settings) = self.print_draft.settings(doc.info.pages.len(), &labels)
                                    && let Ok(bytes) = self.session.print_pdf(temp_id, &settings)
                                {
                                    let mut job = self.print_draft.job(&name);
                                    let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                                    if let Ok(sheets) = print::layout(&sizes, &settings) {
                                        print::add_cups_orientation(&mut job.options, &sheets);
                                    }
                                    if self.print_draft.handling == Handling::Booklet && self.print_draft.printer_two_sided().is_none() {
                                        job.duplex = None;
                                    }
                                    if let Some(ref p) = printer
                                        && self.send_to_printer(p.clone(), bytes, job, Some(*idx)).is_ok()
                                    {
                                        succeeded = true;
                                    }
                                }
                            }
                            self.session.close(temp_id);
                        }
                    }
                }
            }

            if succeeded {
                queued_count += 1;
            } else {
                failed_count += 1;
            }
        }

        if queued_count > 0 {
            self.notify_fmt("Sent {n} documents to printer", &[("n", &queued_count.to_string())]);
        }
        if failed_count > 0 {
            self.notify_fmt("{n} documents could not be printed", &[("n", &failed_count.to_string())]);
        }

        failed_count == 0 && queued_count > 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct BatchPrintFormState;

/// Renders the Batch Print window/dialog.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !app.batch_print.open {
        return;
    }
    match app.batch_print.step {
        BatchPrintStep::SelectFiles => show_file_selection(app, ui),
        BatchPrintStep::Settings => show_print_settings(app, ui),
    }
}

fn show_file_selection(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let selected_count = app.batch_print.items.iter().filter(|i| i.selected).count();
    let total_count = app.batch_print.items.len();
    let total_selected_pages: usize = app.batch_print.items.iter().filter(|i| i.selected).map(|i| i.page_count).sum();

    let mut open = true;
    let mut pick_folder = false;
    let mut add_open_docs = false;
    let mut form_state = BatchPrintFormState;

    let shown = FormDialog::new(tl!("Batch Print"), Bind::bound(&mut form_state))
        .opened(Bind::bound(&mut open))
        .ok_label(tl!("Print settings"))
        .ok_enabled(selected_count > 0)
        .accept_stays_open(true)
        .show(ui, |ui, _, _| {
            let d = &mut app.batch_print;
            let mut remove_idx = None;

            // Toolbar buttons
            ui.horizontal(|ui| {
                #[cfg(not(target_arch = "wasm32"))]
                if Button::new(tl!("Add folder…")).show(ui).clicked() {
                    pick_folder = true;
                }
                if Button::new(tl!("All open documents")).show(ui).clicked() {
                    add_open_docs = true;
                }
                ui.separator();
                if Button::new(tl!("Select all")).show(ui).clicked() {
                    d.select_all(true);
                }
                if Button::new(tl!("Deselect all")).show(ui).clicked() {
                    d.select_all(false);
                }
                if Button::new(tl!("Clear list")).show(ui).clicked() {
                    d.preview_cache.clear();
                    d.items.clear();
                }
            });

            ui.add_space(8.0);

            // Document list
            let max_h = 320.0;
            egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(200.0).auto_shrink([false, false]).show(ui, |ui| {
                if d.items.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(tl!("No documents in batch print list."));
                        ui.label(tl!("Click 'All open documents' or 'Add folder…' to populate files."));
                    });
                    return;
                }

                for (idx, item) in d.items.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        Checkbox::new(Bind::bound(&mut item.selected)).show(ui);
                        if item.is_tab {
                            ui.colored_label(Color32::from_rgb(46, 160, 67), format!("[{}]", tl!("Tab")));
                        } else {
                            ui.colored_label(Color32::from_rgb(56, 139, 253), format!("[{}]", tl!("File")));
                        }
                        ui.label(&item.name);

                        if item.page_count > 0 {
                            let p_text = crate::i18n::fmt(tl!("({n} p.)"), &[("n", &item.page_count.to_string())]);
                            ui.label(RichText::new(p_text).color(ui.visuals().weak_text_color()));
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Button::new("✕").show(ui).clicked() {
                                remove_idx = Some(idx);
                            }
                        });
                    });
                    ui.separator();
                }
            });

            if let Some(idx) = remove_idx {
                d.items.remove(idx);
                d.preview_cache.clear();
            }

            ui.add_space(8.0);
            if total_selected_pages > 0 {
                let text = crate::i18n::fmt(
                    tl!("Selected: {selected} / {total} documents ({pages} pages)"),
                    &[("selected", &selected_count.to_string()), ("total", &total_count.to_string()), ("pages", &total_selected_pages.to_string())],
                );
                ui.label(text);
            } else {
                let text = crate::i18n::fmt(
                    tl!("Selected: {selected} / {total} documents"),
                    &[("selected", &selected_count.to_string()), ("total", &total_count.to_string())],
                );
                ui.label(text);
            }
            true
        });

    if pick_folder {
        app.batch_print_pick_folder();
    }
    if add_open_docs {
        app.batch_print_add_open_documents();
    }

    if shown.closed.is_some() || !open {
        app.batch_print.clear();
        app.batch_print.open = false;
        return;
    }

    if shown.accepted.is_some() || shown.picked == Some(0) {
        app.batch_print.preview_cache.clear();
        app.batch_print.step = BatchPrintStep::Settings;
        app.prepare_batch_print_settings();
    }
}

fn apply_grayscale(img: &mut egui::ColorImage) {
    for c in &mut img.pixels {
        let lum = (f32::from(c.r()) * 0.299 + f32::from(c.g()) * 0.587 + f32::from(c.b()) * 0.114).round() as u8;
        *c = Color32::from_rgba_premultiplied(lum, lum, lum, c.a());
    }
}

fn show_print_settings(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();

    if app.print_draft.handling == Handling::Label {
        let label_crop_ids: Vec<pdfcraft_engine::DocId> = app.batch_print.items.iter().filter(|i| i.selected).filter_map(|i| i.doc_id).collect();
        for id in label_crop_ids {
            app.read_label_crops(id);
        }
    }

    let is_grayscale = app.print_draft.grayscale;
    if app.batch_print.last_grayscale != is_grayscale {
        app.batch_print.preview_cache.clear();
        app.batch_print.last_grayscale = is_grayscale;
    }

    // Extract sizes and labels for all selected items
    let mut all_sizes: Vec<(f64, f64)> = Vec::new();
    let mut all_labels: Vec<String> = Vec::new();
    let mut selected_count = 0usize;

    for item in app.batch_print.items.iter().filter(|i| i.selected) {
        selected_count += 1;
        for (&size, label) in item.page_sizes.iter().zip(&item.page_labels) {
            all_sizes.push(size);
            all_labels.push(label.clone());
        }
    }

    let total_pages = all_sizes.len();

    let mut open = true;
    let mut form_state = BatchPrintFormState;
    let raw_printers = print_ui::listed_printers();
    let printers = print_ui::offered_printers(&raw_printers);

    let back_btn = FooterButton::new(tl!("Back to documents"), DialogRole::Other);

    let can_print = selected_count > 0 && app.print_draft.printer.is_some() && !app.print_draft.papers_loading();
    let shown = FormDialog::new(tl!("Batch Print - Settings"), Bind::bound(&mut form_state))
        .opened(Bind::bound(&mut open))
        .ok_label(tl!("Print"))
        .ok_enabled(can_print)
        .extra_buttons(vec![back_btn])
        .show(ui, |ui, _, _| {
            let summary_text = crate::i18n::fmt(
                tl!("Batch Print: {documents} documents · {pages} total pages"),
                &[("documents", &selected_count.to_string()), ("pages", &total_pages.to_string())],
            );
            ui.label(RichText::new(summary_text).strong());
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                // Left side: settings
                ui.vertical(|ui| {
                    ui.set_width(320.0);

                    // Printer selection
                    let entries: Vec<Entry<String>> = printers.iter().map(|p| Entry::new(p.name.clone(), p.name.clone())).collect();
                    let mut current_printer = app.print_draft.printer.clone().unwrap_or_default();
                    let _ = ComboBox::new(tl!("Printer"), Bind::bound(&mut current_printer), &entries).show(ui);
                    if !current_printer.is_empty() && app.print_draft.printer.as_deref() != Some(&current_printer) {
                        app.print_draft.printer = Some(current_printer.clone());
                        app.print_draft.printer_picked = true;
                        app.print_draft.driver_error = None;
                        app.print_draft.apply_setup();
                        app.print_draft.reset_unoverridden_custom_paper();
                        app.print_draft.apply_printer_papers(Vec::new());
                        #[cfg(not(target_arch = "wasm32"))]
                        app.read_papers_in_background(current_printer);
                    }

                    ui.add_space(6.0);

                    // Copies
                    ui.horizontal(|ui| {
                        ui.label(tl!("Copies:"));
                        ui.add(egui::DragValue::new(&mut app.print_draft.copies).range(1..=999));
                    });

                    // Grayscale & Collate
                    Checkbox::new(Bind::bound(&mut app.print_draft.grayscale)).label(tl!("Grayscale (black and white)")).show(ui);
                    Checkbox::new(Bind::bound(&mut app.print_draft.collate)).label(tl!("Collate")).show(ui);

                    ui.add_space(6.0);
                    let handling_entries = vec![
                        Entry::new("size".to_string(), tl!("Fit to printable area")),
                        Entry::new("poster".to_string(), tl!("Poster")),
                        Entry::new("multiple".to_string(), tl!("Multiple pages per sheet")),
                        Entry::new("booklet".to_string(), tl!("Booklet")),
                    ];
                    let mut current_handling = match app.print_draft.handling {
                        Handling::Size => "size",
                        Handling::Poster => "poster",
                        Handling::Multiple => "multiple",
                        Handling::Booklet => "booklet",
                        Handling::Label => "label",
                    }
                    .to_string();
                    let shown_handling =
                        Segmented::new(tl!("Page Sizing & Handling"), Bind::bound(&mut current_handling), &handling_entries).show(ui);
                    if let Some(picked) = shown_handling.value() {
                        match picked.as_str() {
                            "size" => app.print_draft.handling = Handling::Size,
                            "poster" => app.print_draft.handling = Handling::Poster,
                            "multiple" => app.print_draft.handling = Handling::Multiple,
                            "booklet" => app.print_draft.handling = Handling::Booklet,
                            _ => {}
                        }
                    }
                });

                ui.separator();

                // Right side: preview
                ui.vertical(|ui| {
                    ui.set_min_width(260.0);
                    let d = &mut app.batch_print;
                    let sheet_idx = d.preview_sheet.min(total_pages.saturating_sub(1));
                    ui.horizontal(|ui| {
                        if Button::new("◀").enabled(sheet_idx > 0).show(ui).clicked() {
                            d.preview_sheet = sheet_idx.saturating_sub(1);
                        }
                        let sheet_text =
                            crate::i18n::fmt(tl!("Sheet {s} / {n}"), &[("s", &(sheet_idx + 1).to_string()), ("n", &total_pages.max(1).to_string())]);
                        ui.label(sheet_text);
                        if Button::new("▶").enabled(sheet_idx + 1 < total_pages).show(ui).clicked() {
                            d.preview_sheet = sheet_idx.saturating_add(1);
                        }
                    });

                    ui.add_space(4.0);

                    if !all_sizes.is_empty() {
                        let p = sheet_idx;
                        if !d.preview_cache.contains_key(&p)
                            && let Some((bytes, doc_p)) = find_page_render_source(&d.items, &app.session, p)
                        {
                            let (w, h) = all_sizes.get(p).copied().unwrap_or((595.0, 842.0));
                            let scale = (800.0_f32 / (w.max(h) as f32)).clamp(0.2, 3.0);
                            let config = pdfcraft_render::RenderConfig { system_fonts: true, ..Default::default() };
                            let mut r = pdfcraft_render::PageRenderer::new(bytes, config);
                            let out = r.render(pdfcraft_render::RenderRequest {
                                page: doc_p,
                                kind: pdfcraft_render::RequestKind::Pixels,
                                tile: None,
                                scale,
                                tag: 0,
                                preview: true,
                            });
                            if out.error.is_none() && out.width > 0 && out.height > 0 {
                                let mut img = egui::ColorImage::from_rgba_unmultiplied([out.width as usize, out.height as usize], &out.rgba);
                                if is_grayscale {
                                    apply_grayscale(&mut img);
                                }
                                let handle = ctx.load_texture(format!("batch-sheet-{p}"), img, egui::TextureOptions::LINEAR);
                                const MAX_PREVIEW_CACHE: usize = 6;
                                if d.preview_cache.len() >= MAX_PREVIEW_CACHE
                                    && let Some(&farthest) = d.preview_cache.keys().max_by_key(|&&k| (k as isize - p as isize).abs())
                                {
                                    d.preview_cache.remove(&farthest);
                                }
                                d.preview_cache.insert(p, handle);
                            }
                        }

                        if let Some(texture) = d.preview_cache.get(&p) {
                            let size = texture.size_vec2();
                            let max_preview = egui::vec2(240.0, 320.0);
                            let aspect = size.x / size.y.max(1.0);
                            let preview_size = if aspect > max_preview.x / max_preview.y {
                                egui::vec2(max_preview.x, max_preview.x / aspect)
                            } else {
                                egui::vec2(max_preview.y * aspect, max_preview.y)
                            };
                            ui.image((texture.id(), preview_size));
                        }
                    }
                });
            });

            true
        });

    if shown.picked == Some(1) {
        // "Back to documents" extra button was clicked
        app.batch_print.preview_cache.clear();
        app.batch_print.step = BatchPrintStep::SelectFiles;
        return;
    }

    if shown.closed.is_some() || !open {
        app.batch_print.clear();
        app.batch_print.open = false;
        return;
    }

    if (shown.accepted.is_some() || shown.picked == Some(0)) && app.print_batch_now(&ctx) {
        app.batch_print.clear();
        app.batch_print.open = false;
    }
}

fn find_page_render_source(items: &[BatchPrintItem], session: &pdfcraft_engine::Session, mut page_idx: usize) -> Option<(Arc<Vec<u8>>, usize)> {
    for item in items.iter().filter(|i| i.selected) {
        if page_idx < item.page_sizes.len() {
            let bytes = if let Some(b) = &item.bytes {
                Some(b.clone())
            } else if let Some(path) = &item.path {
                std::fs::read(path).ok().map(Arc::new)
            } else if let Some(id) = item.doc_id
                && let Some(doc) = session.get(id)
            {
                Some(doc.bytes.clone())
            } else {
                None
            };
            return bytes.map(|b| (b, page_idx));
        }
        page_idx -= item.page_sizes.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_print_draft_selection_and_clearing() {
        let mut draft = BatchPrintDraft::default();
        draft.items.push(BatchPrintItem {
            name: "doc1.pdf".to_string(),
            path: None,
            doc_id: None,
            bytes: None,
            page_count: 5,
            page_sizes: vec![(595.0, 842.0); 5],
            page_labels: (1..=5).map(|n| n.to_string()).collect(),
            selected: true,
            is_tab: true,
        });
        draft.items.push(BatchPrintItem {
            name: "doc2.pdf".to_string(),
            path: None,
            doc_id: None,
            bytes: None,
            page_count: 10,
            page_sizes: vec![(595.0, 842.0); 10],
            page_labels: (1..=10).map(|n| n.to_string()).collect(),
            selected: true,
            is_tab: false,
        });

        assert_eq!(draft.items.len(), 2);
        assert_eq!(draft.items.iter().filter(|i| i.selected).map(|i| i.page_count).sum::<usize>(), 15);

        draft.select_all(false);
        assert!(!draft.items[0].selected);
        assert!(!draft.items[1].selected);

        draft.select_only_open();
        assert!(draft.items[0].selected);
        assert!(!draft.items[1].selected);

        draft.settings_initialized = true;
        draft.preview_sheet = 3;
        draft.clear();
        assert!(draft.items.is_empty());
        assert!(!draft.settings_initialized);
        assert_eq!(draft.preview_sheet, 0);
    }
}
