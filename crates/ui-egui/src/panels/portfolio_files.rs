//! The Portfolio files panel: a portfolio's files (ISO 32000-2 §7.11.6), one row each. Choosing
//! a row shows that file in the portfolio's own tab (`crate::app::portfolio`); what else can be
//! done with a file is in the row's context menu, not on the row.

use super::{Align, Rect, Sense, Tokens, human_size, icons, theme, vec2};
use pdfcraft_render::{Attachment, AttachmentSource};

/// What the reader asked of a portfolio's file in the panel.
pub(crate) enum FileAction {
    /// Show the file its name tree keys so in the portfolio's tab.
    Show(String),
    /// Open attachment `index` in a tab of its own (a copy to edit), or save it (`false`).
    Attachment(usize, bool),
}

/// The width of the bar that marks the row shown, beside its background: colour is not the
/// only sign of it.
const MARK_WIDTH: f32 = 3.0;

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
    let mut focus_to = None;
    for (n, &(index, file, key)) in rows.iter().enumerate() {
        let selected = shown == Some(key);
        let is_pdf = file.name.to_lowercase().ends_with(".pdf");
        let row = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
            egui::Frame::NONE
                .fill(if selected { t.selected } else { egui::Color32::TRANSPARENT })
                .corner_radius(4.0)
                .inner_margin(egui::Margin { left: 10, right: 6, top: 6, bottom: 6 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add(icons::image(if is_pdf { "file-text" } else { "paperclip" }, 16.0, if selected { t.accent_text } else { t.icon }));
                        ui.vertical(|ui| {
                            // Not selectable, so a click on the name or the details reaches the row.
                            let font = if selected { theme::semibold(13.0) } else { theme::medium(13.0) };
                            ui.add(egui::Label::new(egui::RichText::new(&file.name).font(font)).truncate().selectable(false));
                            let meta = [file.size.map(human_size), file.description.clone().filter(|d| !d.is_empty())]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join("  ·  ");
                            if !meta.is_empty() {
                                ui.add(egui::Label::new(egui::RichText::new(meta).color(t.text_faint).small()).truncate().selectable(false));
                            }
                        });
                    });
                });
        });
        let response = row.response;
        if selected {
            let r = response.rect;
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(MARK_WIDTH, r.height())), 1.5, t.accent);
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListBoxOption, true, file.name.clone()));
        // Selected, not toggled: the row shown is the list's selection, not a check box.
        ui.ctx().accesskit_node_builder(response.id, |node| node.set_selected(selected));
        if response.clicked() {
            action = Some(FileAction::Show(key.to_owned()));
        }
        if response.has_focus() {
            // ↑ and ↓ are the list's: egui's own arrow navigation would move the keyboard to
            // whatever is nearest that way, the pages included.
            ui.memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
            let (down, up) = ui.input_mut(|i| {
                (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
            });
            if down && n + 1 < rows.len() {
                focus_to = Some(n + 1);
            } else if up && n > 0 {
                focus_to = Some(n - 1);
            }
        }
        // The Menu key or Shift+F10 on the focused row opens the same menu a secondary click does,
        // under the row. A menu opened so stays under the row while it is open: there is no
        // pointer to place it at.
        let keyed = response.id.with("menu-by-key");
        let by_key = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Menu) || (i.modifiers.shift && i.key_pressed(egui::Key::F10)));
        if by_key {
            ui.data_mut(|d| d.insert_temp(keyed, true));
        } else if response.secondary_clicked() {
            ui.data_mut(|d| d.remove::<bool>(keyed));
        }
        let mut menu = egui::Popup::context_menu(&response);
        if by_key {
            menu = menu.open_memory(egui::SetOpenCommand::Bool(true));
        }
        if ui.data(|d| d.get_temp::<bool>(keyed)).unwrap_or(false) {
            menu = menu.anchor(response.rect).align(egui::RectAlign::BOTTOM_START);
        }
        let shown_menu = menu.show(|ui| {
            if is_pdf && ui.button(tl!("Open in a new tab")).clicked() {
                action = Some(FileAction::Attachment(index, true));
                ui.close();
            }
            if ui.button(tl!("Save attachment…")).clicked() {
                action = Some(FileAction::Attachment(index, false));
                ui.close();
            }
        });
        if shown_menu.is_none() {
            ui.data_mut(|d| d.remove::<bool>(keyed));
        }
        responses.push(response);
    }
    if let Some(r) = focus_to.and_then(|n| responses.get(n)) {
        r.request_focus();
        r.scroll_to_me(Some(Align::Center));
    }
    action
}
