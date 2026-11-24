//! Page Properties ▸ Tab Order for the pages the dialog opened on: Use Row Order, Use Column Order, Use Document
//! Structure and Unspecified, as a hosted dialog with one radio row.

use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use pdfcraft_engine::{Edit, TabOrder};

use crate::{PagePropsDraft, PdfCraftApp};

/// The dialog's key: its remembered values and its result are found by it.
const KEY: &str = "page-props";
/// The radio row's key.
const ORDER: &str = "tab_order";

/// The id the radio row stores for `order`.
fn order_id(order: TabOrder) -> &'static str {
    match order {
        TabOrder::Row => "row",
        TabOrder::Column => "column",
        TabOrder::Structure => "structure",
        TabOrder::Annotations => "annotations",
    }
}

/// The order a stored radio id names.
fn order_of(id: &str) -> Option<TabOrder> {
    [TabOrder::Row, TabOrder::Column, TabOrder::Structure, TabOrder::Annotations].into_iter().find(|o| order_id(*o) == id)
}

impl PdfCraftApp {
    /// Opens Page Properties for the pages and document `draft` names; a newer open replaces the old.
    pub(crate) fn open_page_props(&mut self, ctx: &egui::Context, draft: PagePropsDraft) {
        let options = vec![
            (order_id(TabOrder::Row).to_string(), tl!("Use Row Order").into()),
            (order_id(TabOrder::Column).to_string(), tl!("Use Column Order").into()),
            (order_id(TabOrder::Structure).to_string(), tl!("Use Document Structure").into()),
            (order_id(TabOrder::Annotations).to_string(), tl!("Unspecified").into()),
        ];
        let default = draft.before.map(|o| order_id(o).to_string());
        let params = vec![Param::new(ORDER, tl!("Tab Order"), ParamKind::radio(options, default.clone()))];
        let mut fields = DialogFields::new();
        // An empty id represents mixed orders; leaving the field absent picks the first radio.
        fields.set(ORDER, default.unwrap_or_default());
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            let chosen = fields.text(ORDER).and_then(order_of).is_some();
            ParamDialog::new(KEY, tl!("Page Properties"), params.clone()).ok_enabled(chosen).values(Bind::bound(fields))
        });
        self.dialogs.open(dialog);
        self.page_props_draft = Some(draft);
    }

    /// One pass over the dialog: applies the chosen order when it differs from what every page has and the document
    /// is still active and as the dialog opened it. Another document becoming active, or an edit, while the dialog
    /// was open means the pages at those indices may not be the ones chosen: nothing changes and the user is told.
    pub(crate) fn page_props_pass(&mut self, ctx: &egui::Context) {
        let Some(result) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(draft) = self.page_props_draft.take() else { return };
        let DialogResult::Ok(result) = result else { return };
        let Some(order) = result.fields.text(ORDER).and_then(order_of) else { return };
        // An order every page already has adds no undo step.
        if draft.before == Some(order) {
            return;
        }
        let active = self.active_ids().is_some_and(|(_, id)| id == draft.doc);
        if active && self.session.get(draft.doc).is_some_and(|d| d.edit_generation() == draft.generation) {
            self.apply_edit(ctx, Edit::SetTabOrder { pages: draft.pages, order });
        } else {
            self.notify_tr(
                "The document changed while the dialog was open, so nothing was changed. Open Page properties again to set the tab order.",
            );
        }
    }
}
