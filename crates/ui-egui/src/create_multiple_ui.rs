//! Create a PDF ▸ Multiple files: the picked PDFs, images and text files become one new,
//! unsaved document, shown in the page grid: remove pages, insert more files between pages,
//! reorder, then save what the grid shows.

use std::sync::Arc;

use crate::PdfCraftApp;

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

impl PdfCraftApp {
    /// Convert the picked files, join them in the order picked, and open the result in the page
    /// grid. A file that can't be converted is left out (and named in a notice).
    pub(crate) fn stage_create_multiple(&mut self, files: Vec<(String, Vec<u8>)>) {
        let max = pdfcraft_engine::MAX_CREATE_FILES;
        let too_many = files.len() > max;
        let (mut sources, mut refused) = (Vec::new(), Vec::new());
        for (name, bytes) in files.into_iter().take(max) {
            match self.session.convert_to_pdf(&name, &Arc::new(bytes)) {
                Ok((_, pdf)) => sources.push((stem(&name).to_string(), pdf, None)),
                Err(_) => refused.push(name),
            }
        }
        if sources.is_empty() {
            self.notify_tr("None of those files can be made into a PDF; use PDFs, images or .txt files");
            return;
        }
        let bytes = match self.session.combine_ranges(&sources) {
            Ok(bytes) => bytes,
            Err(e) => return self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]),
        };
        let n = sources.len().to_string();
        let message = if !refused.is_empty() {
            crate::i18n::fmt(tl!("Created a PDF from {n} files; left out: {names}"), &[("n", &n), ("names", &refused.join(", "))])
        } else if too_many {
            crate::i18n::fmt(tl!("Created a PDF from the first {n} files"), &[("n", &n)])
        } else {
            crate::i18n::fmt(tl!("Created a PDF from {n} files"), &[("n", &n)])
        };
        let before = self.views.len();
        self.open_created("Combined.pdf", bytes, &message);
        if self.views.len() > before
            && let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
        {
            view.organize = true;
        }
    }
}
