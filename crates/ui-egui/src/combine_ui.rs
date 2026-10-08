//! Combine files: the files to combine, in order, each with the pages to take (all, or a range
//! such as "1-3, 6"); reorder, remove or add files, then Combine.

use std::sync::Arc;

use egui::{Align, Layout};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, icons, widgets};

#[derive(Clone, Debug)]
pub struct CombineFile {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub pages: usize,
    /// The pages to take ("" = all).
    pub range: String,
}

enum RowAction {
    Up(usize),
    Down(usize),
    Remove(usize),
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new("Combine files").font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Files are combined in this order. Leave Pages empty to take every page.").small().color(t.text_faint));
    ui.add_space(8.0);
    let mut action = None;
    let n = app.combine_draft.len();
    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("combine-files").num_columns(4).min_col_width(40.0).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
            ui.label(egui::RichText::new("File").color(t.text_muted));
            ui.label(egui::RichText::new("Pages").color(t.text_muted));
            ui.label("");
            ui.label("");
            ui.end_row();
            for (i, f) in app.combine_draft.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(icons::image("file-text", 16.0, t.icon));
                    ui.add(egui::Label::new(&f.name).truncate());
                    ui.label(egui::RichText::new(format!("{} page{}", f.pages, if f.pages == 1 { "" } else { "s" })).small().color(t.text_faint));
                });
                ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut f.range).hint_text("All pages"))
                    .on_hover_text(format!("Pages of {} to combine, e.g. 1-3, 6", f.name));
                ui.horizontal(|ui| {
                    if ui.add_enabled_ui(i > 0, |ui| icons::button(ui, "chevron-up", 24.0, false, "Move up")).inner.clicked() {
                        action = Some(RowAction::Up(i));
                    }
                    if ui.add_enabled_ui(i + 1 < n, |ui| icons::button(ui, "chevron-down", 24.0, false, "Move down")).inner.clicked() {
                        action = Some(RowAction::Down(i));
                    }
                });
                if icons::button(ui, "trash-2", 24.0, false, &format!("Remove {}", f.name)).clicked() {
                    action = Some(RowAction::Remove(i));
                }
                ui.end_row();
            }
        });
    });
    match action {
        Some(RowAction::Up(i)) => app.combine_draft.swap(i, i - 1),
        Some(RowAction::Down(i)) => app.combine_draft.swap(i, i + 1),
        Some(RowAction::Remove(i)) => {
            app.combine_draft.remove(i);
        }
        None => {}
    }
    ui.add_space(10.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, "Add files…", false).clicked() {
            app.combine_dialog();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ok = app.combine_draft.len() >= 2;
            if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, "Combine", true)).inner.on_disabled_hover_text("Add at least two files").clicked()
            {
                go = true;
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                cancel = true;
            }
        });
    });
    (go, cancel)
}

impl PdfCraftApp {
    /// Add picked files to the Combine files list (and show it).
    pub(crate) fn stage_combine(&mut self, files: Vec<(String, Vec<u8>)>) {
        for (name, bytes) in files {
            let bytes = Arc::new(bytes);
            match pdfcraft_render::inspect(bytes.clone(), None) {
                Ok(info) => self.combine_draft.push(CombineFile { name, bytes, pages: info.pages.len(), range: String::new() }),
                Err(e) => self.notify(format!("Couldn't add {name}: {e}")),
            }
        }
        self.dialog = Some(Dialog::Combine);
    }

    /// Combine the listed files into a new, unsaved document.
    pub fn combine_staged(&mut self) {
        let files = std::mem::take(&mut self.combine_draft);
        if files.is_empty() {
            return;
        }
        let count = files.len();
        let sources: Vec<(String, Arc<Vec<u8>>, Option<String>)> = files
            .into_iter()
            .map(|f| (crate::files::strip_pdf(&f.name).to_string(), f.bytes, Some(f.range).filter(|r| !r.trim().is_empty())))
            .collect();
        match self.session.combine_ranges(&sources) {
            Ok(bytes) => self.open_created("Combined.pdf", bytes, &format!("Combined {count} files")),
            Err(e) => self.notify(format!("Couldn't combine files: {e}")),
        }
    }
}
