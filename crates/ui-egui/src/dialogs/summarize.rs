//! Summarize Comments: the Summarize Options dialog (layout, paper, sort order and the pages
//! without comments) and the summary it makes. The dialog is a command dialog the app's dialog
//! host shows.

use craft::ui::contract::Bind;
use craft::ui::dialog::DialogResult;
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind};
use pdfcraft_engine::{SUMMARY_PAPERS, SummaryLayout, SummarySort};

use crate::PdfCraftApp;

const KEY: &str = "comment.summarize";
const LAYOUT: &str = "layout";
const PAPER: &str = "paper";
/// The paper choice that takes the paper the document is.
const MATCH: &str = "match";
const SORT: &str = "sort";
const PAGES: &str = "pages_without_comments";

/// The Summarize Options dialog over its values; `matched` names the paper "Match the document"
/// is (A4 for an A4 document).
fn build<'a>(fields: &'a mut DialogFields, matched: &str) -> ParamDialog<'a> {
    let layouts = SummaryLayout::ALL.into_iter().map(|l| (l.key().to_string(), tl!(l.name()).into())).collect();
    let layout = ParamKind::radio(layouts, Some(SummaryLayout::default().key().to_string()));
    let match_label = crate::i18n::fmt(tl!("Match the document ({name})"), &[("name", tl!(matched))]);
    let papers =
        std::iter::once((MATCH.to_string(), match_label.into())).chain(SUMMARY_PAPERS.into_iter().map(|p| (p.to_string(), tl!(p).into()))).collect();
    let sorts = SummarySort::ALL.into_iter().map(|s| (s.name().to_string(), tl!(s.name()).into())).collect();
    // Comments only lists comments, not pages.
    let with_pages = SummaryLayout::ALL.into_iter().filter(|l| l.shows_pages()).map(|l| l.key());
    let params = vec![
        Param::new(LAYOUT, tl!("Choose a layout:"), layout),
        Param::new(PAPER, tl!("Paper size:"), ParamKind::choice(papers, Some(MATCH.to_string()))),
        Param::new(SORT, tl!("Sort comments by:"), ParamKind::choice(sorts, None)),
        Param::new(PAGES, tl!("Pages containing no comments"), ParamKind::bool(true)).enabled_when_in(LAYOUT, with_pages),
    ];
    ParamDialog::command(KEY, tl!("Summarize Options"), params).values(Bind::bound(fields)).ok_label(tl!("Create PDF Comment Summary").into())
}

impl PdfCraftApp {
    /// Open Summarize Options with the choices last used.
    pub fn open_summarize(&mut self, ctx: &egui::Context) {
        let matched = self.active_ids().map_or("US Letter", |(_, id)| self.session.summary_paper(id).0);
        let fields = DialogFields::from_pairs([
            (LAYOUT, FieldValue::from(self.summary_layout.key())),
            (PAPER, FieldValue::from(self.summary_paper.unwrap_or(MATCH))),
            (SORT, FieldValue::from(self.summary_sort.name())),
            (PAGES, FieldValue::from(self.summary_pages_without_comments)),
        ]);
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| build(fields, matched));
        self.summarize_dialog = Some(crate::dialogs::host::open(self, dialog));
    }

    /// One pass over the Summarize Options dialog: on OK, remembers the choices and makes the
    /// summary.
    pub fn summarize_pass(&mut self) {
        let Some(id) = self.summarize_dialog else { return };
        match self.dialogs.take_by_id::<HostedParamDialog>(id) {
            None => {}
            Some(DialogResult::Ok(result)) => {
                self.summarize_dialog = None;
                self.take_summary_choices(&result.fields);
                self.summarize_comments();
            }
            Some(_) => self.summarize_dialog = None,
        }
    }

    /// Keeps the dialog's choices; a value it doesn't offer leaves the one before.
    fn take_summary_choices(&mut self, fields: &DialogFields) {
        if let Some(layout) = fields.text(LAYOUT).and_then(SummaryLayout::parse) {
            self.summary_layout = layout;
        }
        if let Some(paper) = fields.text(PAPER) {
            self.summary_paper = SUMMARY_PAPERS.into_iter().find(|p| *p == paper);
        }
        if let Some(sort) = fields.text(SORT).and_then(SummarySort::parse) {
            self.summary_sort = sort;
        }
        if let Some(on) = fields.bool(PAGES) {
            self.summary_pages_without_comments = on;
        }
    }
}
