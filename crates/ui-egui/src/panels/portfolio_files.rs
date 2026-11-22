//! The Portfolio files panel: a portfolio's files (ISO 32000-2 §7.11.6), one row each. Choosing
//! a row shows that file in the portfolio's own tab (`crate::app::portfolio`); what else can be
//! done with a file is in the row's context menu, not on the row.

use super::file_rows::{self, FileRow};
use super::{Tokens, human_size, icons};
use pdfcraft_render::{Attachment, AttachmentSource};

/// What the reader asked of a portfolio's file in the panel.
pub(crate) enum FileAction {
    /// Show the file its name tree keys so in the portfolio's tab.
    Show(String),
    /// Open attachment `index` in a tab of its own (a copy to edit), or save it (`false`).
    Attachment(usize, bool),
}

/// Draw portfolio `files`, marking the one shown (its name tree key, `None` for the portfolio's
/// own pages). ↑ and ↓ move the keyboard between rows, Enter or Space shows the file of the
/// row that has it.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, files: &[Attachment], shown: Option<&str>) -> Option<FileAction> {
    let mut action = None;
    // Only the portfolio's own files: a file attached to a comment on its cover sheet isn't one.
    let rows: Vec<(usize, &Attachment, &str)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match &a.source {
            AttachmentSource::Document { key } => Some((i, a, key.as_str())),
            AttachmentSource::Annotation { .. } => None,
        })
        .collect();
    if rows.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("folder-open", 28.0, t.text_faint));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tl!("This portfolio has no files.")).color(t.text_faint));
        });
        return None;
    }
    let mut responses = Vec::with_capacity(rows.len());
    for &(index, file, key) in &rows {
        let is_pdf = file.name.to_lowercase().ends_with(".pdf");
        let row = FileRow {
            name: &file.name,
            icon: if is_pdf { "file-text" } else { "paperclip" },
            details: vec![file.size.map(human_size).unwrap_or_default(), file.description.clone().unwrap_or_default()],
            selected: shown == Some(key),
            buttons_width: 0.0,
        };
        let input = file_rows::show(
            ui,
            t,
            &row,
            |_| Vec::new(),
            |ui| {
                if is_pdf && ui.button(tl!("Open in a new tab")).clicked() {
                    action = Some(FileAction::Attachment(index, true));
                    ui.close();
                }
                if ui.button(tl!("Save attachment…")).clicked() {
                    action = Some(FileAction::Attachment(index, false));
                    ui.close();
                }
            },
        );
        if input.clicked {
            action = Some(FileAction::Show(key.to_owned()));
        }
        responses.push(input.response);
    }
    file_rows::arrows(ui, &responses);
    action
}
