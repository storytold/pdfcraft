use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogId, DialogResult};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind, ParamResult};
use pdfcraft_engine::{DocId, Edit};

use super::PdfCraftApp;

/// What the open attachment description dialog was opened for: the document at the edit generation it showed, and
/// the attachment's name-tree key and whether it had a description.
#[derive(Clone)]
pub(crate) struct AttachmentOpen {
    pub(crate) doc: DocId,
    pub(crate) generation: u64,
    pub(crate) key: String,
    pub(crate) has_description: bool,
}

impl PdfCraftApp {
    /// Opens the description editor for one attachment; a newer open replaces the old.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_attachment_description(
        &mut self,
        ctx: &egui::Context,
        doc: DocId,
        generation: u64,
        key: String,
        name: String,
        description: String,
        has_description: bool,
    ) {
        let dialog = HostedParamDialog::new(ctx, "attachment-description", DialogFields::new(), move |fields| {
            ParamDialog::new(
                "attachment-description",
                tl!("Edit attachment description"),
                vec![
                    Param::read_only_text("name", tl!("Name"), name.clone()),
                    Param::new("description", tl!("Description"), ParamKind::text(description.clone())),
                ],
            )
            .values(Bind::bound(fields))
        });
        let id: DialogId = self.dialogs.open(dialog);
        self.attachment_description = Some((id, AttachmentOpen { doc, generation, key, has_description }));
    }

    /// One pass over the description dialog: applies its answer, or cancels it when the document it was opened for
    /// changed or stopped being the active one.
    pub(crate) fn attachment_description_pass(&mut self, ctx: &egui::Context) {
        let Some((dialog, open)) = self.attachment_description.clone() else { return };
        match self.dialogs.take_by_id::<HostedParamDialog>(dialog) {
            Some(DialogResult::Ok(result)) => {
                self.attachment_description = None;
                self.apply_attachment_description(ctx, &open, &result);
            }
            Some(_) => self.attachment_description = None,
            None => {
                let active = self.active_ids().is_some_and(|(_, active)| active == open.doc);
                let current = self.session.get(open.doc).is_some_and(|doc| doc.edit_generation() == open.generation);
                if !active || !current {
                    self.dialogs.cancel_by_id(ctx, dialog);
                    self.attachment_description = None;
                }
            }
        }
    }

    /// Edits the description when the trimmed text differs from the attachment's, or is empty over an existing
    /// description, and the document is as the dialog opened it.
    fn apply_attachment_description(&mut self, ctx: &egui::Context, open: &AttachmentOpen, result: &ParamResult) {
        let still_current = self.session.get(open.doc).is_some_and(|doc| doc.edit_generation() == open.generation);
        let active = self.active_ids().is_some_and(|(_, active)| active == open.doc);
        if !still_current || !active {
            self.notify_tr("The document changed while the dialog was open, so nothing was changed. Open the description editor again.");
            return;
        }
        let normalized = result.fields.text("description").unwrap_or_default().trim().to_owned();
        let current = self.session.get(open.doc).and_then(|doc| {
            doc.info
                .attachments
                .iter()
                .find(|attachment| matches!(&attachment.source, pdfcraft_render::AttachmentSource::Document { key } if *key == open.key))
                .map(|attachment| attachment.description.as_deref().unwrap_or_default().trim().to_owned())
        });
        if current.is_some_and(|current| current != normalized || (normalized.is_empty() && open.has_description)) {
            self.apply_edit(ctx, Edit::EditAttachmentDescription { key: open.key.clone(), description: Some(normalized) });
        }
    }
}
