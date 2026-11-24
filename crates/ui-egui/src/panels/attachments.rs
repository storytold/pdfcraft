//! The Attachments panel: the document's embedded files and file-attachment annotations, with Save,
//! Open and Delete on each row and the row's menu.
// PLAN: assumed `super::right::PanelResult` and a `BodyCx` passed to `show` (pdf-chrome-w13 calls `show(app, ui)` today).

use craft::behavior::{DisplayOrder, IconId, ItemSource};
use craft::ui::buttons::Button;
use craft::ui::contract::{BodyCx, CollectionEvent, Entry, SelectMode, Show as _, ShowCollection as _};
use craft::ui::list::{List, ListRow, RowEvent, TrailingFace, TrailingPart};
use craft::ui::menu::Menu;
use craft::ui::primitives::IconSpec;

use super::right::PanelResult;
use super::{empty_state, human_size};
use crate::PdfCraftApp;

const SAVE: TrailingPart = TrailingPart::new(0);
const OPEN: TrailingPart = TrailingPart::new(1);
const DELETE: TrailingPart = TrailingPart::new(2);

/// What the row menu's entries do.
const MENU_OPEN: u8 = 0;
const MENU_SAVE: u8 = 1;
const MENU_DESCRIBE: u8 = 2;
const MENU_DELETE: u8 = 3;

/// The attachments, keyed by position.
struct Attached(usize);

impl DisplayOrder<usize> for Attached {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0).then_some(*key)
    }
}

impl ItemSource<usize> for Attached {}

/// Whether an attachment is a PDF by its name; whether its bytes are is decided where it opens.
fn is_pdf(a: &pdfcraft_render::Attachment) -> bool {
    a.name.to_lowercase().ends_with(".pdf")
}

/// Show the Attachments panel for the active document.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui, cx: &mut BodyCx<'_>) -> PanelResult {
    let mut result = PanelResult::default();
    let Some((_, id)) = app.active_ids() else { return result };
    let Some(doc) = app.session.get(id) else { return result };
    let info = &doc.info;
    let attachable = doc.allows_modification() && doc.read_only_reason.is_none();
    if attachable && Button::new(tl!("Attach a file…").to_string()).show(ui).clicked() {
        result.command = Some("edit.attach");
    }
    if info.attachments.is_empty() {
        empty_state(ui, "paperclip", tl!("This document has no attachments."));
        return result;
    }
    // Only an attachment of the document itself can be deleted here.
    let deletable = |a: &pdfcraft_render::Attachment| match &a.source {
        pdfcraft_render::AttachmentSource::Document { key } if attachable => Some(key.clone()),
        _ => None,
    };
    let detail = |a: &pdfcraft_render::Attachment| {
        let mut parts = vec![a.size.map(human_size).unwrap_or_default()];
        if let pdfcraft_render::AttachmentSource::Annotation { page, .. } = a.source {
            parts.push(crate::i18n::fmt(tl!("on page {label}"), &[("label", info.pages.get(page).map(|p| p.label.as_str()).unwrap_or("?"))]));
        }
        if let Some(d) = &a.description {
            parts.push(d.clone());
        }
        parts.join("  ·  ")
    };
    let out = List::<usize>::new(tl!("Attachments").to_string()).select(SelectMode::Single).natural_height(true).show_items(
        ui,
        &Attached(info.attachments.len()),
        &mut |key, _| {
            let Some(a) = info.attachments.get(*key) else { return ListRow::new("") };
            let mut row = ListRow::new(a.name.clone()).icon(IconSpec::svg(IconId::new("paperclip"))).detail(detail(a)).trailing_part(
                SAVE,
                TrailingFace::Icon(IconSpec::svg(IconId::new("file-down"))),
                tl!("Save attachment…").to_string(),
            );
            if is_pdf(a) {
                row = row.trailing_part(OPEN, TrailingFace::Icon(IconSpec::svg(IconId::new("file-input"))), tl!("Open in a new tab").to_string());
            }
            if deletable(a).is_some() {
                row = row.trailing_part(DELETE, TrailingFace::Icon(IconSpec::svg(IconId::new("trash-2"))), tl!("Delete attachment").to_string());
            }
            row
        },
    );
    // A double click opens a PDF in a new tab, as Acrobat opens an attachment; any other file is saved,
    // since PdfCraft opens only PDFs.
    // The row the open context menu belongs to lives in the panel body's memory, not in egui's temp data.
    let menu_slot = cx.id.with("attachment-menu-row");
    for event in &out.events {
        match event {
            CollectionEvent::Activate(ai) => {
                if let Some(a) = info.attachments.get(*ai) {
                    result.attachment = Some((*ai, is_pdf(a)));
                }
            }
            CollectionEvent::Item(ai, RowEvent::Trailing { part, .. }) => {
                let Some(a) = info.attachments.get(*ai) else { continue };
                if *part == SAVE {
                    result.attachment = Some((*ai, false));
                } else if *part == OPEN {
                    result.attachment = Some((*ai, true));
                } else if *part == DELETE
                    && let Some(key) = deletable(a)
                {
                    result.edit = Some(pdfcraft_engine::Edit::DeleteAttachment { key });
                }
            }
            CollectionEvent::ContextMenu(request) => {
                *cx.memory.shown::<Option<usize>>(menu_slot) = Some(request.key);
            }
            _ => {}
        }
    }
    let row: Option<usize> = *cx.memory.shown::<Option<usize>>(menu_slot);
    let mut describe = None;
    if let Some(ai) = row
        && let Some(a) = info.attachments.get(ai)
    {
        let key = deletable(a);
        let mut entries = Vec::new();
        if is_pdf(a) {
            entries.push(Entry::new(MENU_OPEN, tl!("Open in a new tab").to_string()));
        }
        entries.push(Entry::new(MENU_SAVE, tl!("Save attachment…").to_string()));
        if key.is_some() {
            entries.push(Entry::new(MENU_DESCRIBE, tl!("Edit description").to_string()));
            entries.push(Entry::new(MENU_DELETE, tl!("Delete attachment").to_string()));
        }
        match Menu::context(tl!("Attachment").to_string(), &out.response).pick(ui, &entries).picked {
            Some(MENU_OPEN) => result.attachment = Some((ai, true)),
            Some(MENU_SAVE) => result.attachment = Some((ai, false)),
            Some(MENU_DESCRIBE) => {
                if let Some(key) = key {
                    describe = Some((doc.edit_generation(), key, a.name.clone(), a.description.clone().unwrap_or_default(), a.has_description));
                }
            }
            Some(MENU_DELETE) => {
                if let Some(key) = key {
                    result.edit = Some(pdfcraft_engine::Edit::DeleteAttachment { key });
                }
            }
            _ => {}
        }
    }
    // Opened once the panel's borrows of the document end: the dialog is the app's.
    if let Some((generation, key, name, description, has_description)) = describe {
        let ctx = ui.ctx().clone();
        app.open_attachment_description(&ctx, id, generation, key, name, description, has_description);
    }
    result
}
