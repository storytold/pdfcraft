use std::path::{Path, PathBuf};

use egui::{Align, Color32, Layout, RichText};

use crate::{PdfCraftApp, icons, theme, widgets};

#[derive(Clone, Debug)]
pub struct BatchPrintItem {
    pub name: String,
    pub path: Option<PathBuf>,
    pub doc_id: Option<pdfcraft_engine::DocId>,
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
}

impl BatchPrintDraft {
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
            if let Ok(bytes) = std::fs::read(&path) {
                let arc_bytes = std::sync::Arc::new(bytes);
                if let Ok(info) = pdfcraft_render::inspect(arc_bytes.clone(), None) {
                    for p in info.pages {
                        page_sizes.push((f64::from(p.width), f64::from(p.height)));
                        page_labels.push(p.label);
                    }
                } else if let Some(n) = pdfcraft_engine::source_page_count(&arc_bytes, None) {
                    for i in 0..n {
                        page_sizes.push((595.0, 842.0));
                        page_labels.push((i + 1).to_string());
                    }
                }
            }
            let page_count = page_sizes.len();
            self.items.push(BatchPrintItem {
                name,
                path: Some(path),
                doc_id: None,
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
        app.batch_print.step = BatchPrintStep::Settings;
        app.prepare_batch_print_settings();
    }

    (false, cancel)
}

fn show_print_settings(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    let t = theme::Tokens::get(ui.ctx());

    if app.print_draft.handling == crate::print_ui::Handling::Label {
        let label_crop_ids: Vec<pdfcraft_engine::DocId> = app.batch_print.items.iter().filter(|i| i.selected).filter_map(|i| i.doc_id).collect();
        for id in label_crop_ids {
            app.read_label_crops(id);
        }
    }

    let selected_items: Vec<&BatchPrintItem> = app.batch_print.items.iter().filter(|i| i.selected).collect();
    let selected_count = selected_items.len();

    let mut go_back = false;
    let mut all_sizes: Vec<(f64, f64)> = Vec::new();
    let mut all_labels: Vec<String> = Vec::new();
    let mut page_to_view: Vec<(Option<usize>, usize)> = Vec::new();

    for item in &selected_items {
        let view_idx = item.doc_id.and_then(|id| app.views.iter().position(|v| v.id == id));
        for (p_idx, (&size, label)) in item.page_sizes.iter().zip(&item.page_labels).enumerate() {
            all_sizes.push(size);
            all_labels.push(label.clone());
            page_to_view.push((view_idx, p_idx));
        }
    }

    let total_pages = all_sizes.len();

    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Back to documents"), false).clicked() {
            go_back = true;
        }
        ui.label(RichText::new(format!("Batch Print: {selected_count} documents · {total_pages} total pages")).font(theme::semibold(16.0)));
    });

    if go_back {
        app.batch_print.step = BatchPrintStep::SelectFiles;
        return (false, false);
    }

    ui.add_space(6.0);

    if !all_sizes.is_empty() {
        let rasters = crate::print_ui::preview_rasters(
            &app.print_draft,
            &all_sizes,
            &all_labels,
            ui.ctx().pixels_per_point(),
            ui.ctx().input(|i| i.max_texture_side) as f32,
        );

        for &(p, scale) in &rasters {
            if let Some(&(Some(view_idx), doc_p)) = page_to_view.get(p) {
                let view = &mut app.views[view_idx];
                view.set_print_grayscale(app.print_draft.grayscale);
                view.queue_print_previews(&[(doc_p, scale)]);
            }
        }

        crate::print_ui::body(ui, &mut app.print_draft, &t, &all_sizes, &all_labels, &mut |p| {
            if let Some(&(Some(view_idx), doc_p)) = page_to_view.get(p) {
                let view = &mut app.views[view_idx];
                view.need_thumbnail(doc_p, true);
                view.page_preview(doc_p)
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
