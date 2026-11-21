//! A portfolio's own tab (ISO 32000-2 §7.11.6): the tab keeps the portfolio and shows one of its
//! files at a time, picked in the Portfolio files panel, in place of the cover sheet the
//! portfolio's own pages are. A file shown there is read-only, since an edit can't be saved back
//! into the portfolio yet; Open in a new tab opens a copy to edit, as the Attachments panel does.
//!
//! A file that isn't a PDF is shown as the PDF that Open would make of it (`source_kind`,
//! `convert_to_pdf`): whatever Open converts is shown here too, with no list of its own. An
//! Office document is converted off the frame thread, as Open converts it.
//!
//! Each file the tab has shown keeps its own view, put aside while another is shown, so going
//! back to one comes back to where the reader was in it. The documents those views show stay
//! open in the session until the tab closes.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use pdfcraft_engine::{DocId, SourceKind};

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
    /// The files shown as a conversion (an image, text or an Office document), by key.
    pub(crate) converted: Vec<String>,
    /// The file being converted off the frame thread to be shown, by key, and its answer.
    preparing: Option<Preparing>,
}

/// An Office document of a portfolio being converted to be shown in the portfolio's tab.
struct Preparing {
    key: String,
    name: String,
    answer: Receiver<Result<Arc<Vec<u8>>, String>>,
}

impl PortfolioTab {
    pub(crate) fn new(host: DocId) -> Self {
        Self { host, shown: None, parked: Vec::new(), converted: Vec::new(), preparing: None }
    }

    /// The documents the views put aside show.
    fn parked_docs(&self) -> impl Iterator<Item = DocId> + '_ {
        self.parked.iter().map(|(_, v)| v.id)
    }

    /// The key of the file being converted to be shown, if one is.
    pub(crate) fn preparing(&self) -> Option<&str> {
        self.preparing.as_ref().map(|p| p.key.as_str())
    }
}

/// Why a portfolio's file wasn't shown in the portfolio's tab.
enum Unshown {
    /// The tab or the file is gone, or it is already shown or being prepared.
    Nothing,
    /// PdfCraft can't open this kind of file.
    Unknown(String),
    /// An Office document, to convert off the frame thread: its name and bytes.
    Office(String, Vec<u8>),
    /// The file is encrypted: it opens in a tab of its own, where the password is asked for.
    Password(usize),
    /// The file couldn't be read, converted or opened.
    Failed(String, String),
}

/// The view a portfolio's tab shows next: one put aside before, at its index, or a new one,
/// and whether it is a conversion.
enum Next {
    Parked(usize),
    Opened(Box<Opened>),
}

/// A portfolio's file just opened: its view, the way the file asks to be opened, read before it
/// was made read-only (an `InitialView` is read from the editable document), and whether it is a
/// conversion of another kind of file.
struct Opened {
    view: DocView,
    initial: pdfcraft_engine::InitialView,
    converted: bool,
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
            Ok(()) | Err(Unshown::Nothing | Unshown::Office(..)) => {}
            Err(Unshown::Unknown(name)) => self.notify_fmt(
                "{name} can't be shown in the portfolio: PdfCraft can't open this kind of file. Save it to open it in another app.",
                &[("name", &name)],
            ),
            Err(Unshown::Password(attachment)) => {
                if let Some(host) = host {
                    self.attachment_action(host, attachment, true);
                }
            }
            Err(Unshown::Failed(name, e)) => self.notify_fmt("Couldn't show {name}: {e}", &[("name", &name), ("e", &e)]),
        }
    }

    /// [`Self::show_portfolio_file`] without telling the reader why a file wasn't shown: the
    /// portfolio's initial document, which simply stays in the list (an Office document is
    /// still converted, and shown when it is ready).
    pub(crate) fn show_in_portfolio_tab(&mut self, index: usize, key: Option<&str>) -> bool {
        self.try_show_in_portfolio_tab(index, key).is_ok()
    }

    fn try_show_in_portfolio_tab(&mut self, index: usize, key: Option<&str>) -> Result<(), Unshown> {
        let Some(tab) = self.views.get(index).and_then(|v| v.portfolio.as_ref()) else { return Err(Unshown::Nothing) };
        if tab.shown.as_deref() == key || (key.is_some() && tab.preparing() == key) {
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
            (None, Some(key)) => match self.open_portfolio_file(host, key) {
                Ok(opened) => Next::Opened(opened),
                Err(Unshown::Office(name, bytes)) => {
                    self.convert_in_background(index, key, name, bytes);
                    return Err(Unshown::Nothing);
                }
                Err(e) => return Err(e),
            },
            // The portfolio's own view is put aside whenever a file is shown.
            (None, None) => return Err(Unshown::Nothing),
        };
        self.put_in_portfolio_tab(index, key, next);
        Ok(())
    }

    /// Make `next` the view portfolio tab `index` shows, as the file `key`, putting the one it
    /// showed aside. A conversion still under way for another file is dropped: the reader moved on.
    fn put_in_portfolio_tab(&mut self, index: usize, key: Option<&str>, next: Next) {
        let Some(mut tab) = self.views.get_mut(index).and_then(|v| v.portfolio.take()) else { return };
        tab.preparing = None;
        let (mut view, initial) = match next {
            Next::Parked(i) => (tab.parked.remove(i).1, None),
            Next::Opened(opened) => {
                let Opened { view, initial, converted } = *opened;
                if converted && let Some(key) = key {
                    tab.converted.push(key.to_owned());
                }
                (view, Some(initial))
            }
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
        if let Some(initial) = initial {
            let right = self.right;
            self.apply_initial_view(index, &initial);
            self.right = right;
        }
        // The view shown follows the window's full screen, new or put aside before: full screen
        // changed only the views shown at the time. After the initial view, as on opening, so
        // leaving full screen comes back to the file's own display.
        if let Some(view) = self.views.get_mut(index) {
            if self.full_screen {
                view.enter_presentation();
            } else {
                view.leave_presentation();
            }
        }
    }

    /// Open portfolio `host`'s file `key` as a read-only document without a tab of its own, and
    /// a view of it, with the way it asks to be opened: a PDF as it is, an image or text as the
    /// PDF Open would make of it, marked as a conversion. An Office document comes back to be
    /// converted off the frame thread.
    fn open_portfolio_file(&mut self, host: DocId, key: &str) -> Result<Box<Opened>, Unshown> {
        use pdfcraft_render::OpenError;
        let attachment = self.portfolio_attachment(host, key).ok_or(Unshown::Nothing)?;
        let doc = self.session.get(host).ok_or(Unshown::Nothing)?;
        let att = doc.info.attachments.get(attachment).cloned().ok_or(Unshown::Nothing)?;
        let bytes = pdfcraft_render::attachment_data(&doc.bytes, doc.password.as_deref(), &att)
            .map_err(|e| Unshown::Failed(att.name.clone(), e.to_string()))?;
        match pdfcraft_engine::source_kind(&att.name, &bytes) {
            Some(SourceKind::Pdf) => {}
            Some(SourceKind::Office) => return Err(Unshown::Office(att.name, bytes)),
            Some(SourceKind::Image | SourceKind::Text) => {
                let (_, pdf) =
                    self.session.convert_to_pdf_inline(&att.name, &Arc::new(bytes)).map_err(|e| Unshown::Failed(att.name.clone(), e.to_string()))?;
                return self.open_conversion(&att.name, pdf);
            }
            None => return Err(Unshown::Unknown(att.name)),
        }
        let name = crate::files::attachment_file_name(&att.name);
        let context = self.session.open_context().with_highlight(self.view_defaults.highlight_fields);
        let prepared = match context.prepare_open(name, None, Arc::new(bytes), None) {
            Ok(prepared) => prepared,
            Err(OpenError::NeedsPassword | OpenError::WrongPassword) => return Err(Unshown::Password(attachment)),
            Err(e) => return Err(Unshown::Failed(att.name, e.to_string())),
        };
        let id = self.session.commit_open(prepared);
        self.portfolio_view(id, tl!("it is shown inside a portfolio; open it in a new tab to edit a copy"), false)
    }

    /// Open `pdf`, converted from the portfolio's file `original`, read-only and named as Open
    /// names a conversion (`photo.png` → `photo.pdf`), and a view of it.
    fn open_conversion(&mut self, original: &str, pdf: Arc<Vec<u8>>) -> Result<Box<Opened>, Unshown> {
        let stem = original.rsplit_once('.').map_or(original, |(s, _)| s);
        let name = crate::files::attachment_file_name(&format!("{stem}.pdf"));
        let id = self.session.open(name, None, pdf, None).map_err(|e| Unshown::Failed(original.to_owned(), e.to_string()))?;
        self.portfolio_view(id, tl!("it is a preview converted from a file in a portfolio, which stays as it is"), true)
    }

    /// Make document `id`, a portfolio's file (`converted` from another kind of file or not),
    /// read-only for `reason`, and a view of it with the way it asks to be opened.
    fn portfolio_view(&mut self, id: DocId, reason: &str, converted: bool) -> Result<Box<Opened>, Unshown> {
        // Read while the document is still editable: read-only, it has no structure to read from.
        let initial = self.session.get(id).map(|d| d.initial_view()).unwrap_or_default();
        self.session.set_read_only(id, reason.to_owned());
        self.session.set_reading_colors(id, self.reading_colors);
        let doc = self.session.get(id).ok_or(Unshown::Nothing)?;
        Ok(Box::new(Opened { view: DocView::new(id, &doc.info, self.view_defaults), initial, converted }))
    }

    /// Convert the Office document `name` (`bytes`), portfolio tab `index`'s file `key`, off the
    /// frame thread, as Open does; [`Self::poll_portfolio_conversions`] shows it when it is ready.
    fn convert_in_background(&mut self, index: usize, key: &str, name: String, bytes: Vec<u8>) {
        let (send, answer) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let file = name.clone();
        let run = move || {
            let _ = send.send(pdfcraft_engine::office_to_pdf(&file, &Arc::new(bytes)).map_err(|e| e.to_string()));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        let Some(tab) = self.views.get_mut(index).and_then(|v| v.portfolio.as_mut()) else { return };
        tab.preparing = Some(Preparing { key: key.to_owned(), name: name.clone(), answer });
        #[cfg(target_arch = "wasm32")]
        run();
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = std::thread::Builder::new().name("portfolio-conversion".into()).spawn(run) {
            if let Some(tab) = self.views.get_mut(index).and_then(|v| v.portfolio.as_mut()) {
                tab.preparing = None;
            }
            self.notify_fmt("Couldn't show {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
        }
    }

    /// Show each portfolio tab's Office document whose conversion is ready, or say why it
    /// couldn't be converted. Called every frame.
    pub(crate) fn poll_portfolio_conversions(&mut self) {
        for index in 0..self.views.len() {
            let Some(tab) = self.views.get_mut(index).and_then(|v| v.portfolio.as_mut()) else { continue };
            let Some(preparing) = &tab.preparing else { continue };
            let answer = match preparing.answer.try_recv() {
                Ok(answer) => answer,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err(tl!("the conversion stopped").to_string()),
            };
            let Some(Preparing { key, name, .. }) = tab.preparing.take() else { continue };
            match answer.map_err(|e| Unshown::Failed(name.clone(), e)).and_then(|pdf| self.open_conversion(&name, pdf)) {
                Ok(opened) => self.put_in_portfolio_tab(index, Some(&key), Next::Opened(opened)),
                Err(Unshown::Failed(name, e)) => self.notify_fmt("Couldn't show {name}: {e}", &[("name", &name), ("e", &e)]),
                Err(_) => {}
            }
        }
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
        // Conversions came from the old version too, and one under way is of an old file.
        tab.converted.clear();
        tab.preparing = None;
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
