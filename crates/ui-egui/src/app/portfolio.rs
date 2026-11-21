//! A portfolio's own tab (ISO 32000-2 §7.11.6): the tab keeps the portfolio and shows one of its
//! files at a time, picked in the Portfolio files panel, in place of the cover sheet the
//! portfolio's own pages are. A file shown there is read-only, since an edit can't be saved back
//! into the portfolio yet; Open in a new tab opens a copy to edit, as the Attachments panel does.
//!
//! Each file the tab has shown keeps its own view, put aside while another is shown, so going
//! back to one comes back to where the reader was in it. The documents those views show stay
//! open in the session until the tab closes.

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
use crate::canvas::DocView;

/// What a portfolio's tab holds besides the view it shows (`DocView::portfolio`).
pub(crate) struct PortfolioTab {
    /// The portfolio.
    pub(crate) host: DocId,
    /// The name-tree key of the file shown; `None` for the portfolio's own pages.
    pub(crate) shown: Option<String>,
    /// The views not shown: the portfolio's own and those of the files shown before, by key.
    pub(crate) parked: Vec<(Option<String>, DocView)>,
}

impl PortfolioTab {
    pub(crate) fn new(host: DocId) -> Self {
        Self { host, shown: None, parked: Vec::new() }
    }

    /// The documents the views put aside show.
    fn parked_docs(&self) -> impl Iterator<Item = DocId> + '_ {
        self.parked.iter().map(|(_, v)| v.id)
    }
}

/// Why a portfolio's file wasn't shown in the portfolio's tab.
enum Unshown {
    /// The tab or the file is gone, or it is already shown.
    Nothing,
    /// The file isn't a PDF.
    NotPdf(String),
    /// The file is encrypted: it opens in a tab of its own, where the password is asked for.
    Password(usize),
    /// The file couldn't be read or opened.
    Failed(String, String),
}

/// The view a portfolio's tab shows next: one put aside before, at its index, or a new one.
enum Next {
    Parked(usize),
    Opened(Box<DocView>),
}

impl PdfCraftApp {
    /// The document tab `index` stands for: a portfolio's own tab stands for the portfolio,
    /// whichever of its files it shows.
    pub fn tab_doc(&self, index: usize) -> Option<DocId> {
        let view = self.views.get(index)?;
        Some(view.portfolio.as_ref().map_or(view.id, |p| p.host))
    }

    /// The first tab that stands for document `id` ([`Self::tab_doc`]): its own, or the tab of
    /// the portfolio `id` is, whichever file that tab shows.
    pub(crate) fn tab_of_doc(&self, id: DocId) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.tab_doc(i) == Some(id))
    }

    /// The active tab and its portfolio, when it is a portfolio's tab.
    pub(crate) fn active_portfolio(&self) -> Option<(usize, DocId)> {
        let index = self.active?;
        Some((index, self.views.get(index)?.portfolio.as_ref()?.host))
    }

    /// Whether a tab shows document `id`, or keeps it aside in a portfolio's tab.
    pub(crate) fn doc_in_a_tab(&self, id: DocId) -> bool {
        self.views.iter().any(|v| v.id == id || v.portfolio.as_ref().is_some_and(|p| p.parked_docs().any(|d| d == id)))
    }

    /// Whether portfolio tab `index` keeps aside a document with unsaved changes: the portfolio
    /// itself, edited before one of its files was shown.
    pub(crate) fn hides_unsaved_work(&self, index: usize) -> bool {
        self.views
            .get(index)
            .and_then(|v| v.portfolio.as_ref())
            .is_some_and(|p| p.parked_docs().any(|id| self.session.get(id).is_some_and(|d| d.dirty)))
    }

    /// Bring back the portfolio's own pages in every portfolio tab that keeps unsaved changes
    /// aside, so the "save changes?" question asks about the portfolio, and Save writes it.
    pub(crate) fn show_unsaved_portfolios(&mut self) {
        for index in 0..self.views.len() {
            if self.hides_unsaved_work(index) {
                self.show_in_portfolio_tab(index, None);
            }
        }
    }

    /// Show in portfolio tab `index` the file its portfolio's name tree keys `key` (`None`: the
    /// portfolio's own pages), telling the reader when it can't be shown there. An encrypted
    /// file opens in a tab of its own instead, which asks for its password.
    pub(crate) fn show_portfolio_file(&mut self, index: usize, key: Option<&str>) {
        let host = self.views.get(index).and_then(|v| v.portfolio.as_ref()).map(|p| p.host);
        match self.try_show_in_portfolio_tab(index, key) {
            Ok(()) | Err(Unshown::Nothing) => {}
            Err(Unshown::NotPdf(name)) => {
                self.notify_fmt("{name} can't be shown in the portfolio: only PDF files can. Save it to open it in another app.", &[("name", &name)])
            }
            Err(Unshown::Password(attachment)) => {
                if let Some(host) = host {
                    self.attachment_action(host, attachment, true);
                }
            }
            Err(Unshown::Failed(name, e)) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]),
        }
    }

    /// [`Self::show_portfolio_file`] without telling the reader why a file wasn't shown: the
    /// portfolio's initial document, which simply stays in the list.
    pub(crate) fn show_in_portfolio_tab(&mut self, index: usize, key: Option<&str>) -> bool {
        self.try_show_in_portfolio_tab(index, key).is_ok()
    }

    fn try_show_in_portfolio_tab(&mut self, index: usize, key: Option<&str>) -> Result<(), Unshown> {
        let Some(tab) = self.views.get(index).and_then(|v| v.portfolio.as_ref()) else { return Err(Unshown::Nothing) };
        if tab.shown.as_deref() == key {
            return Err(Unshown::Nothing);
        }
        // What is typed on the pages shown is committed before they are put aside, as before a
        // save, so a view put aside holds no typing that closing could drop; a commit refused
        // (and said why) keeps them shown.
        let current = self.views[index].id;
        if !self.commit_typing_in(current) {
            return Err(Unshown::Nothing);
        }
        let Some(tab) = self.views.get(index).and_then(|v| v.portfolio.as_ref()) else { return Err(Unshown::Nothing) };
        let host = tab.host;
        let next = match (tab.parked.iter().position(|(k, _)| k.as_deref() == key), key) {
            (Some(i), _) => Next::Parked(i),
            (None, Some(key)) => Next::Opened(self.open_portfolio_file(host, key)?),
            // The portfolio's own view is put aside whenever a file is shown.
            (None, None) => return Err(Unshown::Nothing),
        };
        let opened = matches!(next, Next::Opened(_));
        let Some(mut tab) = self.views.get_mut(index).and_then(|v| v.portfolio.take()) else { return Err(Unshown::Nothing) };
        let mut view = match next {
            Next::Parked(i) => tab.parked.remove(i).1,
            Next::Opened(view) => *view,
        };
        let old = &self.views[index];
        // The new view takes the tab's place among the panes; its own key keeps its scroll
        // position and what is drawn over its pages apart from the other files'.
        view.pane.place = old.pane.place;
        view.pane.placed = old.pane.placed;
        view.pane.shown_seq = old.pane.shown_seq;
        let old = std::mem::replace(&mut self.views[index], view);
        tab.parked.push((tab.shown.take(), old));
        tab.shown = key.map(str::to_owned);
        self.views[index].portfolio = Some(tab);
        // A file shown for the first time opens the way it asks to (its layout, magnification
        // and page), as it would in a tab of its own; one shown before keeps where the reader
        // left it. The tab keeps its file list, whatever panel the file asks for.
        if opened && let Some(initial) = self.session.get(self.views[index].id).map(|d| d.initial_view()) {
            let right = self.right;
            self.apply_initial_view(index, &initial);
            self.right = right;
        }
        Ok(())
    }

    /// Open portfolio `host`'s file `key` as a read-only document without a tab of its own, and
    /// a view of it.
    fn open_portfolio_file(&mut self, host: DocId, key: &str) -> Result<Box<DocView>, Unshown> {
        use pdfcraft_render::OpenError;
        let attachment = self.portfolio_attachment(host, key).ok_or(Unshown::Nothing)?;
        let doc = self.session.get(host).ok_or(Unshown::Nothing)?;
        let att = doc.info.attachments.get(attachment).cloned().ok_or(Unshown::Nothing)?;
        let bytes = pdfcraft_render::attachment_data(&doc.bytes, doc.password.as_deref(), &att)
            .map_err(|e| Unshown::Failed(att.name.clone(), e.to_string()))?;
        if pdfcraft_engine::source_kind(&att.name, &bytes) != Some(pdfcraft_engine::SourceKind::Pdf) {
            return Err(Unshown::NotPdf(att.name));
        }
        let name = crate::files::attachment_file_name(&att.name);
        let context = self.session.open_context().with_highlight(self.view_defaults.highlight_fields);
        let prepared = match context.prepare_open(name, None, std::sync::Arc::new(bytes), None) {
            Ok(prepared) => prepared,
            Err(OpenError::NeedsPassword | OpenError::WrongPassword) => return Err(Unshown::Password(attachment)),
            Err(e) => return Err(Unshown::Failed(att.name, e.to_string())),
        };
        let id = self.session.commit_open(prepared);
        self.session.set_read_only(id, tl!("it is shown inside a portfolio; open it in a new tab to edit a copy").to_string());
        self.session.set_reading_colors(id, self.reading_colors);
        let doc = self.session.get(id).ok_or(Unshown::Nothing)?;
        let mut view = Box::new(DocView::new(id, &doc.info, self.view_defaults));
        if self.full_screen {
            view.enter_presentation();
        }
        Ok(view)
    }

    /// The documents a closing portfolio tab put aside, closed with it unless another tab
    /// shows them (split view).
    pub(crate) fn close_parked(&mut self, tab: PortfolioTab) {
        self.close_docs_left(tab.parked_docs().collect());
    }

    /// Portfolio tab `index`, showing the portfolio's own pages, drops the views of the files it
    /// showed and closes their documents: the portfolio was replaced (its file changed on disk),
    /// so a file picked again opens from the new version.
    pub(crate) fn forget_portfolio_files(&mut self, index: usize) {
        let Some(tab) = self.views.get_mut(index).and_then(|v| v.portfolio.as_mut()) else { return };
        let (own, files): (Vec<_>, Vec<_>) = std::mem::take(&mut tab.parked).into_iter().partition(|(key, _)| key.is_none());
        tab.parked = own;
        self.close_docs_left(files.into_iter().map(|(_, view)| view.id).collect());
    }

    /// Close each of `ids` that no tab shows or keeps aside any more.
    fn close_docs_left(&mut self, ids: Vec<DocId>) {
        for id in ids {
            if !self.doc_in_a_tab(id) {
                self.placed_signatures.remove(&id);
                self.forget_recovery(id);
                self.session.close(id);
            }
        }
    }
}
