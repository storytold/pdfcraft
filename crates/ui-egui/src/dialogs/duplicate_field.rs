//! Duplicate Field: copies a form field onto the pages a range names.

use craft::pages::PageRange;
use craft::pages::ui::range::{PageRangeChoices, PageRangeOptions};
use craft::ui::contract::Bind;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, ParamDialog};
use pdfcraft_engine::Edit;

use crate::PdfCraftApp;

/// The key the dialog's values hold the field's name under; it has no row.
const FIELD: &str = "field";

/// Opens Duplicate Field for the field `name`, over All pages.
pub(crate) fn open(app: &mut PdfCraftApp, ctx: &egui::Context, name: String) {
    let Some((i, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let count = doc.info.pages.len();
    let current = app.views.get(i).map_or(0, |v| v.current());
    let mut fields = DialogFields::new();
    fields.set("pages", PageRangeOptions::field_value(&PageRange::default()));
    fields.set(FIELD, name.clone());
    let line = crate::i18n::fmt(tl!("Duplicate \"{name}\" onto:"), &[("name", &name)]);
    let dialog = HostedParamDialog::new(ctx, "form.duplicate_field", fields, move |fields| {
        let rows = vec![PageRangeOptions::param("pages", "", count, current, PageRangeChoices::FromTo)];
        ParamDialog::new("form.duplicate_field", tl!("Duplicate Field"), rows).title_note(line.clone()).values(Bind::bound(fields))
    });
    crate::dialogs::host::open(app, dialog);
}

/// Duplicate Field was confirmed with `fields`: duplicates the field onto the range's pages.
// PLAN: pdf-hub-w2 routes the "form.duplicate_field" result here.
pub(crate) fn finish(ctx: &egui::Context, app: &mut PdfCraftApp, fields: &DialogFields) {
    let Some(name) = fields.text(FIELD).map(str::to_owned) else { return };
    let Some(range) = PageRangeOptions::value(fields, "pages") else { return };
    let Some((i, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let count = doc.info.pages.len();
    let current = app.views.get(i).map_or(0, |v| v.current());
    let pages = range.pages(count, current, &[]);
    app.apply_edit(ctx, Edit::DuplicateField { name, pages });
}
