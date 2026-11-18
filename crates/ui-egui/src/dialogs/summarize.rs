//! Summarize Comments: the Summarize Options dialog (layout and sort order) and the summary it
//! makes. The dialog is a command dialog the app's dialog host shows.
// PLAN: `dialogs.rs` needs `mod summarize;` (pdf-hub-w2 owns it); the pass runs from the host's `results`.

use craft::ui::contract::Bind;
use craft::ui::dialog::DialogResult;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use pdfcraft_engine::SummarySort;

use crate::PdfCraftApp;

const KEY: &str = "comment.summarize";
const LAYOUT: &str = "layout";
const COMMENTS_ONLY: &str = "comments_only";
const SORT: &str = "sort";

/// The Summarize Options dialog over its values.
fn build(fields: &mut DialogFields) -> ParamDialog<'_> {
    let layout = ParamKind::radio(vec![(COMMENTS_ONLY.to_string(), tl!("Comments only").into())], Some(COMMENTS_ONLY.to_string()));
    let sorts = SummarySort::ALL.into_iter().map(|s| (s.name().to_string(), tl!(s.name()).into())).collect();
    let params =
        vec![Param::new(LAYOUT, tl!("Choose a layout:"), layout), Param::new(SORT, tl!("Sort comments by:"), ParamKind::choice(sorts, None))];
    ParamDialog::command(KEY, tl!("Summarize Options"), params).values(Bind::bound(fields)).ok_label(tl!("Create PDF Comment Summary").into())
}

impl PdfCraftApp {
    /// Open Summarize Options, with the sort order last used.
    pub fn open_summarize(&mut self, ctx: &egui::Context) {
        let fields = DialogFields::from_pairs([(LAYOUT, COMMENTS_ONLY), (SORT, self.summary_sort.name())]);
        let dialog = HostedParamDialog::new(ctx, KEY, fields, build);
        // PLAN: assumed `PdfCraftApp::summarize_dialog: Option<DialogId>` (pdf-hub-w1)
        self.summarize_dialog = Some(crate::dialogs::host::open(self, dialog));
    }

    /// One pass over the Summarize Options dialog: on OK, remembers the sort order and makes the
    /// summary.
    pub fn summarize_pass(&mut self) {
        let Some(id) = self.summarize_dialog else { return };
        match self.dialogs.take_by_id::<HostedParamDialog>(id) {
            None => {}
            Some(DialogResult::Ok(result)) => {
                self.summarize_dialog = None;
                if let Some(sort) = result.fields.text(SORT).and_then(SummarySort::parse) {
                    self.summary_sort = sort;
                }
                self.summarize_comments();
            }
            Some(_) => self.summarize_dialog = None,
        }
    }
}
