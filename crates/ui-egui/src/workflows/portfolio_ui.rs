//! Create a PDF ▸ PDF portfolio: pick files of any type, give each an exhibit label, a
//! description and a document date, and bundle them, unchanged, into one PDF portfolio with an
//! indexed cover sheet (a case file or an exhibit packet). The new portfolio opens as an unsaved
//! tab, where the Attachments panel lists its files.

use std::sync::Arc;

use pdfcraft_engine::{ExhibitStyle, PortfolioFile, PortfolioLayout, PortfolioOptions, exhibit_label, parse_document_date};

use crate::PdfCraftApp;
use crate::widgets;

/// One file waiting to go into the portfolio.
#[derive(Clone, Debug)]
pub struct PortfolioRow {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// Last modified, in Unix seconds, when the file came from disk.
    pub modified: Option<i64>,
    pub description: String,
    /// As typed: `2026-03-01`, `3/1/2026` or `March 1, 2026`.
    pub date: String,
}

/// The Create PDF portfolio dialog's settings.
#[derive(Clone, Debug)]
pub struct PortfolioDraft {
    /// Which draft this is, so files picked for one never land in another (see
    /// [`crate::files::FilePurpose::PortfolioAdd`]).
    pub id: u64,
    pub rows: Vec<PortfolioRow>,
    pub title: String,
    /// Lines under the title on the cover sheet: case number, court, prepared by.
    pub note: String,
    pub exhibits: ExhibitStyle,
    pub prefix: String,
    pub layout: PortfolioLayout,
    /// Open on the first file rather than the cover sheet.
    pub open_first: bool,
    /// Why the last Create didn't make the portfolio (a date it couldn't read, say).
    pub error: Option<String>,
}

impl Default for PortfolioDraft {
    fn default() -> Self {
        // Each draft gets a new id; only equality matters, so a process-wide counter does.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            rows: Vec::new(),
            title: String::new(),
            note: String::new(),
            exhibits: ExhibitStyle::Letters,
            prefix: "Exhibit".into(),
            layout: PortfolioLayout::Details,
            open_first: false,
            error: None,
        }
    }
}

/// What the dialog asked for this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PortfolioAsk {
    pub create: bool,
    pub cancel: bool,
    pub add_files: bool,
}

/// The dialog's body.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> PortfolioAsk {
    let mut ask = PortfolioAsk::default();
    ui.heading(tl!("Create PDF portfolio"));
    let Some(d) = app.portfolio_draft.as_mut() else {
        ask.cancel = true;
        return ask;
    };
    ui.label(tl!("The files are bundled unchanged into one PDF, with a cover sheet that lists them."));
    ui.add_space(8.0);

    egui::Grid::new("portfolio-settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        // Each field is named by the label beside it: a hint is only a placeholder.
        let label = ui.label(tl!("Title"));
        ui.add(egui::TextEdit::singleline(&mut d.title).hint_text(tl!("Smith v. Jones: trial exhibits")).desired_width(360.0)).labelled_by(label.id);
        ui.end_row();
        let label = ui.label(tl!("Cover sheet note"));
        ui.add(egui::TextEdit::multiline(&mut d.note).hint_text(tl!("Case number, court, prepared by")).desired_rows(2).desired_width(360.0))
            .labelled_by(label.id);
        ui.end_row();
        ui.label(tl!("Exhibit labels"));
        ui.horizontal(|ui| {
            widgets::radio_value(ui, &mut d.exhibits, ExhibitStyle::Letters, tl!("A, B, C"));
            widgets::radio_value(ui, &mut d.exhibits, ExhibitStyle::Numbers, tl!("1, 2, 3"));
            widgets::radio_value(ui, &mut d.exhibits, ExhibitStyle::None, tl!("None"));
        });
        ui.end_row();
        let label = ui.label(tl!("Label word"));
        ui.add_enabled(d.exhibits != ExhibitStyle::None, egui::TextEdit::singleline(&mut d.prefix).hint_text(tl!("Exhibit")).desired_width(160.0))
            .labelled_by(label.id);
        ui.end_row();
    });

    ui.add_space(8.0);
    ui.label(crate::i18n::fmt(tl!("Files: {count}"), &[("count", &d.rows.len().to_string())]));
    let (mut up, mut down, mut remove) = (None, None, None);
    egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("portfolio-files").num_columns(5).striped(true).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.strong(tl!("Label"));
            ui.strong(tl!("File"));
            ui.strong(tl!("Description"));
            ui.strong(tl!("Document date"));
            ui.label("");
            ui.end_row();
            let last = d.rows.len().saturating_sub(1);
            for (i, row) in d.rows.iter_mut().enumerate() {
                ui.label(exhibit_label(d.exhibits, &d.prefix, i).unwrap_or_else(|| i.saturating_add(1).to_string()));
                ui.label(&row.name).on_hover_text(crate::panels::human_size(row.bytes.len()));
                // A row's fields and buttons say which file they belong to, starting with their
                // column or action, so a screen reader tells the rows apart.
                let named = |text: &str| crate::i18n::fmt(text, &[("name", &row.name)]);
                ui.add(egui::TextEdit::singleline(&mut row.description).hint_text(tl!("What it is")).desired_width(170.0))
                    .accessible_name(named(tl!("Description for {name}")));
                ui.add(egui::TextEdit::singleline(&mut row.date).hint_text("2026-03-01").desired_width(96.0))
                    .accessible_name(named(tl!("Document date for {name}")));
                let (up_name, down_name, remove_name) = (named(tl!("Move {name} up")), named(tl!("Move {name} down")), named(tl!("Remove {name}")));
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).accessible_name(up_name).on_hover_text(tl!("Move up")).clicked() {
                        up = Some(i);
                    }
                    if ui.add_enabled(i < last, egui::Button::new("↓").small()).accessible_name(down_name).on_hover_text(tl!("Move down")).clicked()
                    {
                        down = Some(i);
                    }
                    if ui.add(egui::Button::new("✕").small()).accessible_name(remove_name).on_hover_text(tl!("Remove")).clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
    });
    // The buttons only offer moves that stay in the list; the checks keep it so regardless.
    let len = d.rows.len();
    if let Some(i) = up.filter(|&i| i > 0 && i < len) {
        d.rows.swap(i, i - 1);
    } else if let Some(i) = down.filter(|&i| i.saturating_add(1) < len) {
        d.rows.swap(i, i + 1);
    } else if let Some(i) = remove.filter(|&i| i < len) {
        d.rows.remove(i);
    }
    ask.add_files = widgets::pill_button(ui, tl!("Add files…"), false).clicked();

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(tl!("Show files as"));
        widgets::radio_value(ui, &mut d.layout, PortfolioLayout::Details, tl!("Details"));
        widgets::radio_value(ui, &mut d.layout, PortfolioLayout::Tile, tl!("Tiles"));
    });
    ui.add(widgets::checkbox(&mut d.open_first, tl!("Open on the first file instead of the cover sheet")));
    if let Some(error) = &d.error {
        ui.add_space(4.0);
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.add_space(12.0);
    let ready = !d.rows.is_empty();
    ui.horizontal(|ui| {
        ask.create = ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Create"), true)).inner.clicked();
        ask.cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    ask
}

impl PdfCraftApp {
    /// Create a PDF ▸ PDF portfolio: ask for the files (any type).
    pub fn create_portfolio_dialog(&mut self) {
        self.pick_files(crate::files::FilePurpose::Portfolio, true);
    }

    /// The dialog's Add files: pick more files for the draft that is open now.
    pub(crate) fn add_portfolio_files(&mut self) {
        if let Some(id) = self.portfolio_draft.as_ref().map(|d| d.id) {
            self.pick_files(crate::files::FilePurpose::PortfolioAdd(id), true);
        }
    }

    /// Files picked for a portfolio. Those picked from the menu start the dialog; those from a
    /// dialog's Add files join that dialog only while it is still open, and are dropped once it
    /// was made, cancelled or replaced, so a slow read never reopens a dialog the person closed.
    pub(crate) fn portfolio_files_picked(&mut self, purpose: crate::files::FilePurpose, files: Vec<(String, Vec<u8>, Option<i64>)>) {
        if let crate::files::FilePurpose::PortfolioAdd(id) = purpose {
            let open = self.dialog == Some(crate::Dialog::CreatePortfolio);
            if !open || self.portfolio_draft.as_ref().map(|d| d.id) != Some(id) {
                return;
            }
        }
        self.begin_portfolio(files);
    }

    /// Add picked files to the portfolio being made, opening the dialog for the first ones.
    /// Each file is `(name, bytes, modified)`.
    pub fn begin_portfolio(&mut self, files: Vec<(String, Vec<u8>, Option<i64>)>) {
        if files.is_empty() {
            return;
        }
        let draft = self.portfolio_draft.get_or_insert_with(PortfolioDraft::default);
        draft.rows.extend(files.into_iter().map(|(name, bytes, modified)| PortfolioRow {
            name,
            bytes: Arc::new(bytes),
            modified,
            description: String::new(),
            date: String::new(),
        }));
        draft.error = None;
        self.dialog = Some(crate::Dialog::CreatePortfolio);
    }

    /// Make the portfolio the dialog describes and open it as a new, unsaved tab. A date that
    /// can't be read, or anything else that stops it, keeps the dialog open with the reason.
    pub(crate) fn finish_portfolio(&mut self) {
        let Some(mut draft) = self.portfolio_draft.take() else { return };
        // Each date is checked first, so the message names the file to fix.
        let checked = draft.rows.iter().try_for_each(|row| parse_document_date(&row.date).map(|_| ()).map_err(|e| format!("{}: {e}", row.name)));
        let created = checked.and_then(|()| {
            let files: Vec<PortfolioFile> = draft
                .rows
                .iter()
                .map(|row| PortfolioFile {
                    name: row.name.clone(),
                    bytes: row.bytes.clone(),
                    description: Some(row.description.clone()).filter(|s| !s.trim().is_empty()),
                    document_date: Some(row.date.clone()).filter(|s| !s.trim().is_empty()),
                    modified: row.modified,
                })
                .collect();
            let title = if draft.title.trim().is_empty() { tl!("PDF Portfolio").to_string() } else { draft.title.trim().to_string() };
            let options = PortfolioOptions {
                title,
                note: Some(draft.note.clone()).filter(|s| !s.trim().is_empty()),
                exhibits: draft.exhibits,
                prefix: draft.prefix.clone(),
                layout: draft.layout,
                initial: draft.open_first.then_some(0),
            };
            self.session.create_portfolio(&files, &options).map(|bytes| (options.title, bytes)).map_err(|e| e.to_string())
        });
        let result = created.and_then(|(title, bytes)| {
            let name = format!("{}.pdf", crate::files::file_stem_from_user(&title));
            self.open_created_bytes(&name, Ok(bytes))
        });
        match result {
            Ok(()) => {
                let n = draft.rows.len().to_string();
                self.notify_fmt("Created a PDF portfolio of {n} files", &[("n", &n)]);
            }
            Err(e) => {
                draft.error = Some(e);
                self.portfolio_draft = Some(draft);
                self.dialog = Some(crate::Dialog::CreatePortfolio);
            }
        }
    }
}
