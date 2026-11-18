//! Page Properties ▸ Tab Order for the pages the dialog opened on: Use Row Order, Use Column Order, Use Document
//! Structure and Unspecified, as a hosted dialog with one radio row.

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
        let params = vec![Param::new(ORDER, tl!("Tab Order"), ParamKind::radio(options, default))];
        let dialog = HostedParamDialog::new(ctx, KEY, DialogFields::new(), move |_| ParamDialog::new(KEY, tl!("Page Properties"), params.clone()));
        self.dialogs.open(dialog);
        self.page_props_draft = Some(draft);
    }

    /// One pass over the dialog: applies the chosen order when it differs from what every page has and the document
    /// is as the dialog opened it.
    pub(crate) fn page_props_pass(&mut self, ctx: &egui::Context, _ctx: &egui::Context) {
        let Some(result) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(draft) = self.page_props_draft.take() else { return };
        let DialogResult::Ok(result) = result else { return };
        let Some(order) = result.fields.text(ORDER).and_then(order_of) else { return };
        // An order every page already has adds no undo step.
        if draft.before == Some(order) {
            return;
        }
        let current = self.session.get(draft.doc).is_some_and(|d| d.edit_generation() == draft.generation);
        let Some(index) = self.views.iter().position(|v| v.id == draft.doc) else { return };
        if current {
            self.active = Some(index);
            self.apply_edit(ctx, Edit::SetTabOrder { pages: draft.pages, order });
        }
    }
}
