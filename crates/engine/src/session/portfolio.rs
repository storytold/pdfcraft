//! Create a PDF ▸ Portfolio: several files of any type bundled, unchanged, into one PDF portfolio
//! (ISO 32000-2 §7.11.6) with a cover sheet that indexes them, as for a case file or an exhibit
//! packet. The files are embedded as they are, never converted, so a Word draft or an email stays
//! itself inside the portfolio.

use pdfcraft_organize::{ExhibitStyle, PortfolioItem, PortfolioLayout, PortfolioSpec, exhibit_label};

use super::{Arc, EditError, Session};

/// One file to put in a portfolio.
#[derive(Clone, Debug, Default)]
pub struct PortfolioFile {
    /// The file name (any folder part is dropped), which is also its name in the portfolio.
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// What the file is ("Signed lease", "Deposition transcript"); shown in the file list and on
    /// the cover sheet.
    pub description: Option<String>,
    /// The document's own date (signed, filed, served) as the person typed it: `2026-03-01`,
    /// `3/1/2026` or `March 1, 2026`. See [`parse_document_date`].
    pub document_date: Option<String>,
    /// The file's last modification, in Unix seconds; `None` uses the time of creation.
    pub modified: Option<i64>,
}

/// How a portfolio is labelled and shown.
#[derive(Clone, Debug)]
pub struct PortfolioOptions {
    /// The cover sheet's heading and the document title ("Smith v. Jones: Trial Exhibits").
    pub title: String,
    /// Optional lines under the heading on the cover sheet (case number, court, prepared by).
    pub note: Option<String>,
    pub exhibits: ExhibitStyle,
    /// The word before each exhibit mark ("Exhibit" gives "Exhibit A").
    pub prefix: String,
    pub layout: PortfolioLayout,
    /// The file (0-based, in list order) presented first; `None` presents the cover sheet.
    pub initial: Option<usize>,
}

impl Default for PortfolioOptions {
    fn default() -> Self {
        Self {
            title: "PDF Portfolio".into(),
            note: None,
            exhibits: ExhibitStyle::Letters,
            prefix: "Exhibit".into(),
            layout: PortfolioLayout::Details,
            initial: None,
        }
    }
}

/// A document date the person typed, as a calendar day: ISO (`2026-03-01`), US numeric
/// (`3/1/2026`, `03-01-2026`) or written out (`March 1, 2026`, `Mar 1 2026`, `1 March 2026`).
/// `Ok(None)` for blank, and an error naming the text for anything else.
pub fn parse_document_date(text: &str) -> Result<Option<chrono::NaiveDate>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    const FORMATS: [&str; 9] = ["%Y-%m-%d", "%m/%d/%Y", "%m-%d-%Y", "%B %d, %Y", "%B %d %Y", "%b %d, %Y", "%b %d %Y", "%d %B %Y", "%d %b %Y"];
    FORMATS
        .iter()
        .find_map(|f| chrono::NaiveDate::parse_from_str(text, f).ok())
        .map(Some)
        .ok_or_else(|| format!("{text:?} is not a date; write it like 2026-03-01, 3/1/2026 or March 1, 2026"))
}

impl Session {
    /// A new PDF portfolio of `files`, in order: a cover sheet titled and indexed by `options`,
    /// with each file embedded unchanged and listed with its exhibit label, description and
    /// dates. A viewer that shows portfolios presents the file list; any other viewer shows the
    /// cover sheet and the files in its Attachments panel.
    pub fn create_portfolio(&self, files: &[PortfolioFile], options: &PortfolioOptions) -> Result<Arc<Vec<u8>>, EditError> {
        if files.is_empty() {
            return Err(EditError::Source("a portfolio needs at least one file".into()));
        }
        if files.len() > pdfcraft_organize::MAX_PORTFOLIO_FILES {
            return Err(EditError::Source(format!("a portfolio holds at most {} files", pdfcraft_organize::MAX_PORTFOLIO_FILES)));
        }
        if let Some(i) = options.initial
            && i >= files.len()
        {
            return Err(EditError::Source(format!("there is no file {} to open first", i.saturating_add(1))));
        }
        let dates = files
            .iter()
            .map(|f| parse_document_date(f.document_date.as_deref().unwrap_or("")).map_err(|e| EditError::Source(format!("{}: {e}", f.name))))
            .collect::<Result<Vec<_>, _>>()?;
        let title = options.title.trim();
        let title = if title.is_empty() { "PDF Portfolio" } else { title };
        let labels: Vec<Option<String>> = (0..files.len()).map(|i| exhibit_label(options.exhibits, &options.prefix, i)).collect();

        let (mut doc, _escaped) = pdfcraft_create::from_text_escaped(
            title,
            &cover_sheet(title, options.note.as_deref(), files, &labels, &dates),
            pdfcraft_create::LETTER,
            11.0,
        )?;

        let now = self.now();
        let mut items = Vec::with_capacity(files.len());
        for ((file, label), date) in files.iter().zip(labels).zip(&dates) {
            let modified = file.modified.or(now).map(pdfcraft_cos::pdf_date);
            let key = pdfcraft_organize::add_attachment(
                &mut doc,
                &file.name,
                &file.bytes,
                file.description.as_deref(),
                pdfcraft_organize::mime_for_name(&file.name),
                modified.as_deref(),
            )?;
            items.push(PortfolioItem { key, exhibit: label, document_date: date.map(|d| d.format("D:%Y%m%d").to_string()) });
        }
        let initial = options.initial.and_then(|i| items.get(i)).map(|item| item.key.clone());
        pdfcraft_organize::set_portfolio(&mut doc, &PortfolioSpec { items, layout: options.layout, initial })?;
        self.write_new(&doc)
    }
}

/// The cover sheet's text: the title and note, a line on what a portfolio is, and the index.
fn cover_sheet(title: &str, note: Option<&str>, files: &[PortfolioFile], labels: &[Option<String>], dates: &[Option<chrono::NaiveDate>]) -> String {
    let mut text = format!("{title}\n");
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        text.push_str(note);
        text.push('\n');
    }
    let count = if files.len() == 1 { "1 document".to_string() } else { format!("{} documents", files.len()) };
    text.push_str(&format!(
        "\nThis PDF portfolio holds {count}. Each one is attached to this file unchanged: open it in a PDF \
         reader that shows portfolios, or open the Attachments panel, to read them.\n\nIndex\n"
    ));
    for (i, file) in files.iter().enumerate() {
        let name = file.name.rsplit(['/', '\\']).next().unwrap_or(&file.name).trim();
        let head = match labels.get(i).cloned().flatten() {
            Some(label) => format!("{label}: {name}"),
            None => format!("{}. {name}", i.saturating_add(1)),
        };
        text.push('\n');
        text.push_str(&head);
        text.push('\n');
        let mut detail = Vec::new();
        if let Some(d) = file.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            detail.push(d.to_string());
        }
        if let Some(date) = dates.get(i).copied().flatten() {
            detail.push(date.format("%B %-d, %Y").to_string());
        }
        if !detail.is_empty() {
            text.push_str("    ");
            text.push_str(&detail.join(" - "));
            text.push('\n');
        }
    }
    text
}
