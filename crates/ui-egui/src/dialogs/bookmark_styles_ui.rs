//! Bookmarks ▸ New bookmarks from text styles (#1693): for documents whose headings aren't
//! tagged, a style per bookmark level (the chapters' font and size for level 1, the sections'
//! for level 2, …), and every line set in it becomes a bookmark of that level
//! ([`pdfcraft_engine::text_style_bookmarks`]).

use pdfcraft_engine::text_style_bookmarks::{TextStyle, TextStyleUse};
use pdfcraft_engine::{DocId, Edit};

use craft::ui::dialog::{Alert, DialogResult, HostedAlert, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;

use crate::PdfCraftApp;
use crate::side_panels::{PanelId, set_panel_visible};

/// The levels offered: deeper outlines are rare, and each level needs a style of its own.
const LEVELS: usize = 4;
/// The dialog's key: its result is found by it.
const KEY: &str = "bookmarks-from-text-styles";
/// The id of the "no style" entry of a level's select.
const NONE: &str = "none";

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

/// The key of level `n`'s select.
fn level_key(n: usize) -> String {
    format!("level{n}")
}

impl PdfCraftApp {
    /// Open New bookmarks from text styles for document `id`: its styles are read now, and the
    /// largest is offered for level 1 when there is more than one (headings are larger than the
    /// text around them; a single style has no headings to tell apart).
    pub fn open_text_style_bookmarks(&mut self, ctx: &egui::Context, id: DocId) {
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
        let draft = &self.text_style_draft;
        if draft.styles.is_empty() {
            // A form dialog always has two buttons; a notice with only Close is an alert.
            let alert =
                Alert::new(tl!("New bookmarks from text styles"), tl!("This document has no text to make bookmarks from.")).ok_label(tl!("Close"));
            self.dialogs.open(HostedAlert::new(alert));
            return;
        }
        let mut options: Vec<(String, Label)> = vec![(NONE.to_string(), tl!("None").into())];
        options.extend(draft.styles.iter().enumerate().map(|(i, u)| (i.to_string(), Label::from(style_label(u)))));
        let style_ids: Vec<String> = (0..draft.styles.len()).map(|i| i.to_string()).collect();
        let params: Vec<Param> = (0..LEVELS)
            .map(|level| {
                let name = crate::i18n::fmt(tl!("Level {n}"), &[("n", &(level + 1).to_string())]);
                let default = Some(draft.levels.get(level).copied().flatten().map_or_else(|| NONE.to_string(), |i| i.to_string()));
                let param = Param::new(level_key(level), name, ParamKind::choice(options.clone(), default));
                // A level takes a style only once the one above it has one.
                if level == 0 { param } else { param.enabled_when_in(level_key(level - 1), style_ids.clone()) }
            })
            .collect();
        let mut fields = DialogFields::new();
        for (level, slot) in draft.levels.iter().enumerate() {
            let value = slot.map_or_else(|| NONE.to_string(), |i| i.to_string());
            fields.set(level_key(level), value);
        }
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            ParamDialog::new(KEY, tl!("New bookmarks from text styles"), params.clone())
                .ok_label(tl!("Create").into())
                .values(craft::ui::contract::Bind::bound(fields))
        });
        self.dialogs.open(dialog);
    }

    /// One pass over the dialog: Create makes the bookmarks and shows them in the Bookmarks panel.
    pub(crate) fn bookmarks_from_styles_pass(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = self.dialogs.get_by_id::<HostedParamDialog>(dialog_id(KEY)) {
            let fields = dialog.fields();
            for (n, slot) in self.text_style_draft.levels.iter_mut().enumerate() {
                *slot = fields.text(&level_key(n)).and_then(|id| id.parse::<usize>().ok());
            }
        }
        let Some(result) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let mut draft = std::mem::take(&mut self.text_style_draft);
        let DialogResult::Ok(result) = result else { return };
        for (n, slot) in draft.levels.iter_mut().enumerate() {
            *slot = result.fields.text(&level_key(n)).and_then(|id| id.parse::<usize>().ok());
        }
        // A level left without a style ends the list: `chosen` stops at the first gap.
        let levels = draft.chosen();
        if levels.is_empty() {
            return;
        }
        // The document the styles came from, even if another tab is active by now.
        if let Some(i) = draft.doc.and_then(|id| self.views.iter().position(|v| v.id == id)) {
            self.active = Some(i);
            if self.apply_edit(ctx, Edit::BookmarksFromTextStyles { levels }) {
                set_panel_visible(self, PanelId::Bookmarks, true);
            }
        }
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
