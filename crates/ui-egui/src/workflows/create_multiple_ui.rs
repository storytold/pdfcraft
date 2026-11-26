//! Create a PDF from multiple files. Combine opens one unsaved document in the page grid;
//! Separate writes one PDF per convertible source without opening tabs.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use craft::docpath::names::{CopyNumbering, safe_file_stem, unique_file_name};
use craft::host::WriteIntent;
use pdfcraft_engine::{EditError, SourceKind};

use crate::PdfCraftApp;

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// Why Create Multiple stopped early: a picked file couldn't be read, or the PDF couldn't be made.
enum Stop {
    Read { name: String, error: String },
    Build(EditError),
}

impl From<EditError> for Stop {
    fn from(e: EditError) -> Self {
        Stop::Build(e)
    }
}

impl PdfCraftApp {
    /// Convert each selected non-PDF file into its own PDF. Existing outputs and duplicate input
    /// names receive numbered names, so a batch never silently replaces an earlier file.
    pub(crate) fn stage_create_separate<I>(&mut self, files: I, dir: Option<PathBuf>)
    where
        I: IntoIterator<Item = Result<(String, Vec<u8>), (String, String)>>,
    {
        let mut used = HashSet::new();
        let mut made = 0usize;
        let mut refused = Vec::new();
        for file in files {
            let (name, bytes) = match file {
                Ok(file) => file,
                Err((name, error)) => {
                    refused.push(format!("{name} ({error})"));
                    continue;
                }
            };
            let (kind, pdf) = match self.session.convert_to_pdf_inline(&name, &Arc::new(bytes)) {
                Ok(converted) => converted,
                Err(error) => {
                    refused.push(format!("{name} ({error})"));
                    continue;
                }
            };
            if kind == SourceKind::Pdf {
                refused.push(format!("{name} ({})", tl!("already a PDF")));
                continue;
            }
            let base = safe_file_stem(stem(&name), "pdf");
            let base = if base.is_empty() { "document" } else { &base };
            let filename = unique_file_name(&format!("{base}.pdf"), CopyNumbering::Parenthesized, |candidate| {
                used.contains(&candidate.to_lowercase()) || dir.as_ref().is_some_and(|d| self.host.files().exists(&d.join(candidate)))
            });
            used.insert(filename.to_lowercase());
            // Without a folder (the web) the write is a download.
            let path = dir.as_ref().map_or_else(|| PathBuf::from(&filename), |d| d.join(&filename));
            match self.host.files().write(&path, &pdf, WriteIntent::SaveAs) {
                Ok(_) => made += 1,
                Err(error) => refused.push(format!("{filename} ({error})")),
            }
        }
        if made > 0 {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(dir) = &dir {
                self.notify_fmt("Wrote {n} files to {dir}", &[("n", &made.to_string()), ("dir", &dir.display().to_string())]);
            }
            #[cfg(target_arch = "wasm32")]
            self.notify(crate::i18n::trn(crate::i18n::current(), made as u64, "Downloaded {n} PDF", "Downloaded {n} PDFs"));
        }
        if !refused.is_empty() {
            self.notify(crate::i18n::fmt(tl!("Left out: {names}"), &[("names", &refused.join(", "))]));
        }
    }

    /// Convert the picked files, join them in the order picked, and open the result in the page
    /// grid. A file that can't be converted is left out (and named in a notice); a file that can't be
    /// read stops the run. Files are read as their turn comes, so one file's bytes are held at a time.
    pub(crate) fn stage_create_multiple<I>(&mut self, files: I)
    where
        I: IntoIterator<Item = Result<(String, Vec<u8>), (String, String)>>,
    {
        let (mut refused, mut converted) = (Vec::new(), 0usize);
        let mut files = files.into_iter();
        let session = &self.session;
        let sources = std::iter::from_fn(|| {
            loop {
                match files.next()? {
                    Ok((name, bytes)) => match session.convert_to_pdf_inline(&name, &Arc::new(bytes)) {
                        Ok((_, pdf)) => {
                            converted += 1;
                            return Some(Ok((stem(&name).to_string(), pdf, None)));
                        }
                        Err(_) => refused.push(name),
                    },
                    Err((name, error)) => return Some(Err(Stop::Read { name, error })),
                }
            }
        });
        let bytes = match session.combine_each(sources) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.notify_tr("None of those files can be made into a PDF; use PDFs, images or .txt files");
                return;
            }
            Err(Stop::Read { name, error }) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &error)]);
                return;
            }
            Err(Stop::Build(e)) => {
                self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let n = converted.to_string();
        let message = if !refused.is_empty() {
            crate::i18n::fmt(tl!("Created a PDF from {n} files; left out: {names}"), &[("n", &n), ("names", &refused.join(", "))])
        } else {
            crate::i18n::fmt(tl!("Created a PDF from {n} files"), &[("n", &n)])
        };
        let before = self.views.len();
        self.open_created("Combined.pdf", bytes, &message);
        if self.views.len() > before
            && let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
        {
            view.open_organize();
        }
    }
}
