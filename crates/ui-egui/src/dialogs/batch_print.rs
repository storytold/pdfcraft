use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align, Color32, Layout, RichText};

use crate::{PdfCraftApp, icons, theme, widgets};

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
    pub step: BatchPrintStep,
    pub items: Vec<BatchPrintItem>,
    pub preview_cache: HashMap<usize, egui::TextureHandle>,
    pub last_grayscale: bool,
}

impl BatchPrintDraft {
    pub fn clear(&mut self) {
        self.step = BatchPrintStep::SelectFiles;
        self.items.clear();
        self.preview_cache.clear();
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

/// Renders the Batch Print dialog. Returns (go_print, cancel).
pub fn dialog_batch_print(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    match app.batch_print.step {
        BatchPrintStep::SelectFiles => show_file_selection(ui, app),
        BatchPrintStep::Settings => show_print_settings(ui, app),
    }
}

fn show_file_selection(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    ui.label(RichText::new(tl!("Batch Print")).font(theme::semibold(18.0)));
    ui.add_space(8.0);

    // Toolbar buttons
    ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        if widgets::pill_button(ui, tl!("Add folder…"), false).clicked() {
            app.batch_print_pick_folder();
        }
        if widgets::pill_button(ui, tl!("All open documents"), false).clicked() {
            app.batch_print_add_open_documents();
        }
        ui.separator();
        if widgets::pill_button(ui, tl!("Select all"), false).clicked() {
            app.batch_print.select_all(true);
        }
        if widgets::pill_button(ui, tl!("Deselect all"), false).clicked() {
            app.batch_print.select_all(false);
        }
        if widgets::pill_button(ui, tl!("Clear list"), false).clicked() {
            app.batch_print.preview_cache.clear();
            app.batch_print.items.clear();
        }
    });

    ui.add_space(8.0);

    // Document list
    let mut remove_index = None;
    let max_h = 320.0;
    egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(200.0).auto_shrink([false, false]).show(ui, |ui| {
        if app.batch_print.items.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(tl!("No documents in batch print list."));
                ui.label(tl!("Click 'All open documents' or 'Add folder…' to populate files."));
            });
            return;
        }

        for (idx, item) in app.batch_print.items.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(widgets::checkbox_without_text(&mut item.selected));
                if item.is_tab {
                    ui.colored_label(Color32::from_rgb(46, 160, 67), "[Tab]");
                } else {
                    ui.colored_label(Color32::from_rgb(56, 139, 253), "[File]");
                }
                ui.label(&item.name);

                if item.page_count > 0 {
                    ui.label(RichText::new(format!("({} p.)", item.page_count)).color(ui.visuals().weak_text_color()));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::button(ui, "x", 16.0, false, tl!("Remove")).clicked() {
                        remove_index = Some(idx);
                    }
                });
            });
            ui.separator();
        }
    });

    if let Some(idx) = remove_index {
        app.batch_print.items.remove(idx);
        app.batch_print.preview_cache.clear();
    }

    ui.add_space(10.0);

    // Footer
    let selected_count = app.batch_print.items.iter().filter(|i| i.selected).count();
    let total_count = app.batch_print.items.len();
    let total_selected_pages: usize = app.batch_print.items.iter().filter(|i| i.selected).map(|i| i.page_count).sum();

    let mut go_settings = false;
    let mut cancel = false;

    ui.horizontal(|ui| {
        if total_selected_pages > 0 {
            ui.label(format!("Selected: {selected_count} / {total_count} documents ({total_selected_pages} pages)"));
        } else {
            ui.label(format!("Selected: {selected_count} / {total_count} documents"));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(selected_count > 0, |ui| widgets::pill_button(ui, tl!("Print settings"), true)).inner.clicked() {
                go_settings = true;
            }

            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });

    if go_settings {
        app.batch_print.preview_cache.clear();
        app.batch_print.step = BatchPrintStep::Settings;
        app.prepare_batch_print_settings();
    }

    (false, cancel)
}

fn apply_grayscale(img: &mut egui::ColorImage) {
    for c in &mut img.pixels {
        let lum = (f32::from(c.r()) * 0.299 + f32::from(c.g()) * 0.587 + f32::from(c.b()) * 0.114).round() as u8;
        *c = Color32::from_rgba_premultiplied(lum, lum, lum, c.a());
    }
}

fn show_print_settings(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    let t = theme::Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();

    if app.print_draft.handling == crate::print_ui::Handling::Label {
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

    let mut go_back = false;
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Back to documents"), false).clicked() {
            go_back = true;
        }
        ui.label(RichText::new(format!("Batch Print: {selected_count} documents · {total_pages} total pages")).font(theme::semibold(16.0)));
    });

    if go_back {
        app.batch_print.preview_cache.clear();
        app.batch_print.step = BatchPrintStep::SelectFiles;
        return (false, false);
    }

    ui.add_space(6.0);

    if !all_sizes.is_empty() {
        crate::print_ui::body(ui, &mut app.print_draft, &t, &all_sizes, &all_labels, &mut |p| {
            if let Some(handle) = app.batch_print.preview_cache.get(&p) {
                return Some(handle.id());
            }

            let (bytes, doc_p) = find_page_render_source(&app.batch_print.items, &app.session, p)?;

            let (w, h) = all_sizes.get(p).copied().unwrap_or((595.0, 842.0));
            let scale = (800.0 / (w.max(h) as f32)).clamp(0.2, 3.0);

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
                let mut img = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                if is_grayscale {
                    apply_grayscale(&mut img);
                }
                let handle = ctx.load_texture(format!("batch-sheet-{p}"), img, egui::TextureOptions::LINEAR);
                let id = handle.id();
                app.batch_print.preview_cache.insert(p, handle);
                Some(id)
            } else {
                None
            }
        })
    } else {
        let dummy_sizes = vec![(595.0, 842.0)];
        let dummy_labels = vec!["1".to_string()];
        crate::print_ui::body(ui, &mut app.print_draft, &t, &dummy_sizes, &dummy_labels, &mut |_| None)
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

        draft.items.clear();
        assert!(draft.items.is_empty());
    }
}
