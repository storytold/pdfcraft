//! A row for a file attached to a document, as the Attachments and Portfolio files panels list
//! them: an icon, the file's name and a line of details, buttons at its end, and a menu. The row
//! is a list option the keyboard reaches: ↑ and ↓ move between rows ([`arrows`]), Enter or
//! Space activates it, and the Menu key or Shift+F10 opens its menu under it.

use super::{Align, Layout, Rect, Sense, Tokens, icons, theme, vec2};

/// The width of the bar that marks the selected row, beside its background: colour is not the
/// only sign of it.
const MARK_WIDTH: f32 = 3.0;

/// What a row shows.
pub(crate) struct FileRow<'a> {
    pub(crate) name: &'a str,
    pub(crate) icon: &'static str,
    /// The details under the name, joined with " · "; empty parts are left out.
    pub(crate) details: Vec<String>,
    /// The list's selection (the file shown), drawn and reported as selected.
    pub(crate) selected: bool,
    /// The width the buttons at the row's end take, kept free of the name.
    pub(crate) buttons_width: f32,
}

/// What the reader did with a row this frame.
pub(crate) struct RowInput {
    /// The row itself, for [`arrows`].
    pub(crate) response: egui::Response,
    /// A primary click on the row, from the pointer or the keyboard (Enter or Space).
    pub(crate) clicked: bool,
    /// Enter or Space on the focused row, or a double click: what opens the row's file.
    pub(crate) activated: bool,
}

/// Draw `row`. `buttons` draws the row's buttons from its end and returns them, so a secondary
/// click on one opens the row's menu too; `menu` fills that menu.
pub(crate) fn show(
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &FileRow<'_>,
    buttons: impl FnOnce(&mut egui::Ui) -> Vec<egui::Response>,
    menu: impl FnOnce(&mut egui::Ui),
) -> RowInput {
    let selected = row.selected;
    let mut drawn = Vec::new();
    let scope = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
        egui::Frame::NONE
            .fill(if selected { t.selected } else { egui::Color32::TRANSPARENT })
            .corner_radius(4.0)
            .inner_margin(egui::Margin { left: 10, right: 6, top: 6, bottom: 6 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(icons::image(row.icon, 16.0, if selected { t.accent_text } else { t.icon }));
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - row.buttons_width);
                        // Not selectable, so a click on the name or the details reaches the row.
                        let font = if selected { theme::semibold(13.0) } else { theme::medium(13.0) };
                        ui.add(egui::Label::new(egui::RichText::new(row.name).font(font)).truncate().selectable(false));
                        let details = row.details.iter().filter(|d| !d.is_empty()).map(String::as_str).collect::<Vec<_>>().join("  ·  ");
                        if !details.is_empty() {
                            ui.add(egui::Label::new(egui::RichText::new(details).color(t.text_faint).small()).truncate().selectable(false));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| drawn = buttons(ui));
                });
            });
    });
    let response = scope.response;
    if selected {
        let r = response.rect;
        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(MARK_WIDTH, r.height())), 1.5, t.accent);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListBoxOption, true, row.name));
    // Selected, not toggled: the row is the list's selection, not a check box.
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_selected(selected));
    if response.has_focus() {
        // ↑ and ↓ are the list's: egui's own arrow navigation would move the keyboard to whatever
        // is nearest that way, the pages included.
        ui.memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
    }
    let keyed = response.clicked() && !response.clicked_by(egui::PointerButton::Primary);
    let input = RowInput { clicked: response.clicked(), activated: keyed || response.double_clicked(), response: response.clone() };
    // A secondary click on one of the row's buttons is one on the row: the union keeps the row's
    // id, so it is one menu either way. Only the row's own response says what was clicked.
    let target = drawn.into_iter().fold(response.clone(), |row, button| row | button);
    // The Menu key or Shift+F10 on the focused row opens the menu a secondary click does, under
    // the row. A menu opened so stays under the row while it is open: there is no pointer to
    // place it at.
    let by_key_id = response.id.with("menu-by-key");
    let by_key = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Menu) || (i.modifiers.shift && i.key_pressed(egui::Key::F10)));
    if by_key {
        ui.data_mut(|d| d.insert_temp(by_key_id, true));
    } else if target.secondary_clicked() {
        ui.data_mut(|d| d.remove::<bool>(by_key_id));
    }
    let mut popup = egui::Popup::context_menu(&target);
    if by_key {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    if ui.data(|d| d.get_temp::<bool>(by_key_id)).unwrap_or(false) {
        popup = popup.anchor(response.rect).align(egui::RectAlign::BOTTOM_START);
    }
    if popup.show(menu).is_none() {
        ui.data_mut(|d| d.remove::<bool>(by_key_id));
    }
    input
}

/// ↑ and ↓ on the focused one of `rows` move the keyboard to the row above or below it.
pub(crate) fn arrows(ui: &mut egui::Ui, rows: &[egui::Response]) {
    let Some(at) = rows.iter().position(egui::Response::has_focus) else { return };
    let (down, up) =
        ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
    let to = if down && at + 1 < rows.len() {
        at + 1
    } else if up && at > 0 {
        at - 1
    } else {
        return;
    };
    rows[to].request_focus();
    rows[to].scroll_to_me(Some(Align::Center));
}
