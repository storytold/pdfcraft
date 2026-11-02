//! Bookmarks ▸ New bookmarks from text styles (#1693): for documents whose headings aren't
//! tagged, a style per bookmark level (the chapters' font and size for level 1, the sections'
//! for level 2, …), and every line set in it becomes a bookmark of that level
//! ([`pdfcraft_engine::text_style_bookmarks`]).

use pdfcraft_engine::text_style_bookmarks::{TextStyle, TextStyleUse};
use pdfcraft_engine::{DocId, Edit};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The levels offered: deeper outlines are rare, and each level needs a style of its own.
const LEVELS: usize = 4;

/// What the dialog shows and what has been chosen in it.
#[derive(Debug, Default)]
pub struct TextStyleDraft {
    /// The document the styles were read from.
    pub doc: Option<DocId>,
    /// Its text styles, the largest first.
    pub styles: Vec<TextStyleUse>,
    /// The style (an index into `styles`) for each level, level 1 first; a level without one
    /// ends the list.
    pub levels: [Option<usize>; LEVELS],
}

impl TextStyleDraft {
    /// The styles for the chosen levels, level 1 first, up to the first level left without one.
    pub fn chosen(&self) -> Vec<TextStyle> {
        self.levels.iter().map_while(|l| l.and_then(|i| self.styles.get(i)).map(|u| u.style.clone())).collect()
    }
}

impl PdfCraftApp {
    /// Open New bookmarks from text styles for document `id`: its styles are read now, and the
    /// largest is offered for level 1 when there is more than one (headings are larger than the
    /// text around them; a single style has no headings to tell apart).
    pub fn open_text_style_bookmarks(&mut self, id: DocId) {
        // The style list includes samples copied from the document's text. Enforce the PDF's
        // copy permission before extracting any of that text, not only when creating bookmarks.
        let can_copy = self.session.get(id).is_some_and(|d| d.permissions().is_none_or(|p| p.copy()));
        if !can_copy {
            self.notify_tr("The document's security settings don't allow copying text and images");
            return;
        }
        let styles = self.session.get(id).map(|d| d.text_styles()).unwrap_or_default();
        let first = (styles.len() > 1).then_some(0);
        self.text_style_draft = TextStyleDraft { doc: Some(id), styles, levels: [first, None, None, None] };
        self.dialog = Some(crate::Dialog::BookmarksFromTextStyles);
    }
}

/// How a style is named in the lists: font, size, bold or italic, how many lines, and a sample.
fn style_label(u: &TextStyleUse) -> String {
    let mut s = crate::i18n::fmt(tl!("{font}, {size} pt"), &[("font", &u.style.font), ("size", &format!("{}", u.style.size))]);
    if u.style.bold {
        s.push_str(", ");
        s.push_str(tl!("bold"));
    }
    if u.style.italic {
        s.push_str(", ");
        s.push_str(tl!("italic"));
    }
    let lines = if u.lines == 1 { tl!("1 line").to_string() } else { crate::i18n::fmt(tl!("{n} lines"), &[("n", &u.lines.to_string())]) };
    let sample: String = u.sample.chars().take(40).collect();
    let more = if sample.chars().count() < u.sample.chars().count() { "…" } else { "" };
    format!("{s} · “{sample}{more}” ({lines})")
}

/// The dialog's contents and its buttons. Returns whether it closes; Create makes the
/// bookmarks and shows them in the Bookmarks panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("New bookmarks from text styles")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let d = &mut app.text_style_draft;
    let id = d.doc;
    if d.styles.is_empty() {
        ui.label(tl!("This document has no text to make bookmarks from."));
        ui.add_space(12.0);
        return widgets::pill_button(ui, tl!("Close"), true).clicked();
    }
    ui.label(
        egui::RichText::new(tl!("Every line set in the style chosen for a level becomes a bookmark of that level, under a new first bookmark."))
            .color(t.text_muted),
    );
    ui.add_space(8.0);
    let none = tl!("None");
    egui::Grid::new("text_style_levels").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        for level in 0..LEVELS {
            // A level takes a style only once the one above it has one.
            let open = level == 0 || d.levels.get(level - 1).copied().flatten().is_some();
            let label = ui.label(crate::i18n::fmt(tl!("Level {n}"), &[("n", &(level + 1).to_string())]));
            ui.add_enabled_ui(open, |ui| {
                let Some(slot) = d.levels.get_mut(level) else { return };
                let shown = slot.and_then(|i| d.styles.get(i)).map_or_else(|| none.to_string(), style_label);
                craft_ui::ComboBox::from_id_salt(("text_style_level", level))
                    .selected_text(shown)
                    .width(460.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(slot, None, none);
                        for (i, u) in d.styles.iter().enumerate() {
                            ui.selectable_value(slot, Some(i), style_label(u));
                        }
                    })
                    .response
                    .labelled_by(label.id);
            });
            ui.end_row();
        }
    });
    // A level left without a style ends the list: the ones below it don't count.
    if let Some(gap) = d.levels.iter().position(Option::is_none) {
        d.levels.iter_mut().skip(gap).for_each(|l| *l = None);
    }
    let levels = d.chosen();
    ui.add_space(16.0);
    let mut close = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(!levels.is_empty(), |ui| widgets::pill_button(ui, tl!("Create"), true)).inner.clicked() {
            close = true;
            // The document the styles came from, even if another tab is active by now.
            if let Some(i) = id.and_then(|id| app.views.iter().position(|v| v.id == id)) {
                app.active = Some(i);
                if app.apply_edit(Edit::BookmarksFromTextStyles { levels }) {
                    app.right = Some(crate::RightPanel::Bookmarks);
                }
            }
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            close = true;
        }
    });
    close
}
