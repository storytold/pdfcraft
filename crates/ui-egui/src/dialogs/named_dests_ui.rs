//! Named destinations (ISO 32000-2 §12.3.2.3): the document's `/Names /Dests`
//! list — a name, the page it opens and the view it opens there — with add,
//! update and delete.
//!
//! Every change is an engine edit ([`Edit::SetNamedDestination`],
//! [`Edit::DeleteNamedDestination`]) through [`PdfCraftApp::apply_edit`], so a
//! shared session sends it as `dest.add`/`dest.update`/`dest.delete` and the
//! replica applies what the room accepted: the dialog never writes to the
//! document itself. In a room a `commenter` or a `viewer` is refused by the
//! room and the form says so; outside one the edit is the ordinary undoable
//! step.

use pdfcraft_engine::{BookmarkDest, Edit};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The destination types of Table 149, with the number of values each takes.
const VIEW_TYPES: [(&str, usize); 8] = [("XYZ", 3), ("Fit", 0), ("FitH", 1), ("FitV", 1), ("FitB", 0), ("FitBH", 1), ("FitBV", 1), ("FitR", 4)];

/// The dialog's form: a name, the page it opens and the view it opens there.
#[derive(Debug, Clone)]
pub struct NamedDestsDraft {
    /// The document the form was opened for.
    pub doc: Option<pdfcraft_engine::DocId>,
    /// The name; one the document already has is updated, one it lacks is added.
    pub name: String,
    /// The 1-based page the form points at.
    pub page: usize,
    /// The view type (a Table 149 name).
    pub kind: String,
    /// The type's values as typed: a number, `null`, or blank for a hole (the
    /// reader keeps its position or zoom there).
    pub values: String,
    /// Why the last change was refused, if it was.
    pub error: Option<String>,
}

impl Default for NamedDestsDraft {
    fn default() -> Self {
        Self { doc: None, name: String::new(), page: 1, kind: "XYZ".into(), values: String::new(), error: None }
    }
}

impl PdfCraftApp {
    /// Open the named destinations of document `id`, with the form empty.
    pub fn open_named_destinations(&mut self, id: pdfcraft_engine::DocId) {
        self.named_dests_draft = NamedDestsDraft { doc: Some(id), ..NamedDestsDraft::default() };
        self.dialog = Some(crate::Dialog::NamedDestinations);
    }
}

/// Load an existing destination into the form, for an update.
fn load(draft: &mut NamedDestsDraft, name: &str, page: Option<usize>, view: Option<&(Vec<u8>, Vec<Option<f64>>)>) {
    draft.name = name.to_owned();
    draft.page = page.map_or(1, |page| page.saturating_add(1));
    if let Some((kind, values)) = view {
        draft.kind = String::from_utf8_lossy(kind).into_owned();
        draft.values = values.iter().map(|value| value.map_or_else(|| "null".to_owned(), |v| v.to_string())).collect::<Vec<_>>().join(", ");
    } else {
        draft.kind = "XYZ".into();
        draft.values = String::new();
    }
    draft.error = None;
}

/// The values typed into the form, checked against the type's own count. Blank
/// text is every value left as a hole; a `/FitR` needs all four of its numbers.
fn values_of(draft: &NamedDestsDraft) -> Result<Vec<Option<f64>>, String> {
    let Some((kind, arity)) = VIEW_TYPES.iter().find(|(kind, _)| *kind == draft.kind) else {
        return Err(tl!("The view type is not one this document can write.").to_owned());
    };
    let typed = draft.values.trim();
    let values = if typed.is_empty() {
        vec![None; *arity]
    } else {
        let mut values = Vec::new();
        for part in typed.split(',') {
            let part = part.trim();
            if part.is_empty() || part.eq_ignore_ascii_case("null") {
                values.push(None);
            } else {
                let number = part.parse::<f64>().map_err(|_| crate::i18n::fmt(tl!("{value} is not a number."), &[("value", part)]))?;
                if !number.is_finite() {
                    return Err(crate::i18n::fmt(tl!("{value} is not a number."), &[("value", part)]));
                }
                values.push(Some(number));
            }
        }
        values
    };
    if values.len() != *arity {
        return Err(crate::i18n::fmt(tl!("A {kind} destination takes {n} values."), &[("kind", kind), ("n", &arity.to_string())]));
    }
    if *kind == "FitR" && values.iter().any(Option::is_none) {
        return Err(tl!("A FitR destination needs all four values.").to_owned());
    }
    Ok(values)
}

/// A destination as the list shows it: its view type and values.
fn view_label(view: Option<&(Vec<u8>, Vec<Option<f64>>)>) -> String {
    let Some((kind, values)) = view else { return tl!("The reader's position").to_owned() };
    let kind = String::from_utf8_lossy(kind).into_owned();
    if values.is_empty() {
        return kind;
    }
    let values: Vec<String> =
        values.iter().map(|value| value.map_or_else(|| "null".to_owned(), |v| format!("{}", (v * 100.0).round() / 100.0))).collect();
    format!("{kind} [{}]", values.join(", "))
}

/// The dialog's contents and its buttons. Returns whether it closes.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Named destinations")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let Some(id) = app.named_dests_draft.doc else { return widgets::pill_button(ui, tl!("Close"), true).clicked() };
    let rows = app.session.get(id).map(|document| document.named_destinations()).unwrap_or_default();
    ui.label(
        egui::RichText::new(tl!("A name a reader can jump to: where it opens, as a page and a view. A link or a bookmark can point at it."))
            .color(t.text_muted),
    );
    ui.add_space(8.0);

    // The list: one row per name, with a way to delete it or load it for an update.
    let mut load_row = None;
    let mut delete = None;
    egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
        if rows.is_empty() {
            ui.label(egui::RichText::new(tl!("This document has no named destinations.")).color(t.text_muted));
        }
        egui::Grid::new("named_dests").num_columns(3).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
            for row in &rows {
                if ui.link(&row.name).clicked() {
                    load_row = Some((row.name.clone(), row.page, row.view.clone()));
                }
                let page = row.page.map_or_else(|| "—".to_owned(), |page| crate::i18n::fmt(tl!("Page {n}"), &[("n", &(page + 1).to_string())]));
                ui.label(page);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(view_label(row.view.as_ref())).color(t.text_muted));
                    if ui.button(tl!("Delete")).clicked() {
                        delete = Some(row.name.clone());
                    }
                });
                ui.end_row();
            }
        });
    });
    if let Some((name, page, view)) = load_row {
        load(&mut app.named_dests_draft, &name, page, view.as_ref());
    }
    if let Some(name) = delete {
        // Every change goes through the edit, so a session sends `dest.delete`
        // and the replica applies what the room accepted.
        if let Some(i) = app.views.iter().position(|v| v.id == id) {
            app.active = Some(i);
            if app.apply_edit(Edit::DeleteNamedDestination { name: name.clone() }) {
                app.named_dests_draft.error = None;
            } else {
                app.named_dests_draft.error = Some(tl!("The change was not applied.").to_owned());
            }
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    // The form: a name, a page and a view.
    let draft = &mut app.named_dests_draft;
    egui::Grid::new("named_dest_form").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(340.0).hint_text(tl!("chapter-1")));
        ui.end_row();
        ui.label(tl!("Page"));
        ui.add(egui::DragValue::new(&mut draft.page).range(1..=1_000_000));
        ui.end_row();
        ui.label(tl!("View"));
        ui.horizontal(|ui| {
            craft_ui::ComboBox::from_id_salt("named_dest_kind").selected_text(draft.kind.clone()).width(90.0).show_ui(ui, |ui| {
                for (kind, _) in VIEW_TYPES {
                    ui.selectable_value(&mut draft.kind, kind.to_owned(), kind);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut draft.values).desired_width(200.0).hint_text(tl!("blank keeps the reader's position")));
        });
        ui.end_row();
    });
    if let Some(error) = &draft.error {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(error.clone()).color(ui.visuals().error_fg_color));
    }

    let name = draft.name.trim().to_owned();
    let page = draft.page;
    let kind = draft.kind.clone().into_bytes();
    let values = values_of(draft);
    let exists = rows.iter().any(|row| row.name == name);
    let pages = app.session.get(id).map_or(0, |document| document.info.pages.len());
    let ready = !name.is_empty() && (1..=pages).contains(&page) && values.is_ok();
    ui.add_space(16.0);
    let mut close = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Close"), false).clicked() {
            close = true;
        }
        let label = if exists { tl!("Update") } else { tl!("Add") };
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            let Some(i) = app.views.iter().position(|v| v.id == id) else { return };
            match values {
                // A view the form cannot write is said where the form is, and
                // nothing is sent.
                Err(error) => app.named_dests_draft.error = Some(error),
                Ok(values) => {
                    app.active = Some(i);
                    let dest = BookmarkDest::View { page: page.saturating_sub(1), kind, values };
                    let accepted = app.apply_edit(Edit::SetNamedDestination { name, dest });
                    app.named_dests_draft.error = (!accepted).then(|| tl!("The change was not applied.").to_owned());
                }
            }
        }
    });
    close
}
