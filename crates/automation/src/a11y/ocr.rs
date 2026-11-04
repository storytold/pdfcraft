use super::{Args, Automation, Result, ToolError, Value, child, failed, json, write_atomic};

impl Automation {
    /// Recognition settings from the arguments: `dpi` (clamped to 72–600), `language` (refused
    /// unless the models read it) and `skip_text_pages`, each left at its default when absent.
    pub(crate) fn ocr_settings(&self, a: &Args) -> Result<pdfcraft_engine::ocr::OcrSettings> {
        let mut settings = pdfcraft_engine::ocr::OcrSettings::default();
        if let Some(d) = a.opt_num("dpi")? {
            settings.dpi = d.clamp(72.0, 600.0) as f32;
        }
        if let Some(l) = a.opt_str("language")? {
            if !pdfcraft_engine::ocr::LANGUAGES.iter().any(|x| x.0 == l) {
                return Err(ToolError::InvalidArgs(format!("unsupported language {l:?}")));
            }
            settings.language = l.into();
        }
        if let Some(s) = a.opt_bool("skip_text_pages")? {
            settings.skip_text_pages = s;
        }
        Ok(settings)
    }

    /// Recognizes text in each of `paths` and writes the result under the same file name in
    /// `folder`, reporting per file its output, word count and skipped pages, or its error. A
    /// path outside the root refuses the whole call before any work starts.
    pub(crate) fn ocr_recognize_files(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let folder = self.resolve(a.str("folder")?, true)?;
        // A path that leaves the root refuses the whole call before any work starts, like the
        // other tools (the write check passes missing files, which stay per-file errors below).
        let paths = a.strs("paths")?;
        for p in &paths {
            self.resolve_for(p, crate::file_access::Access::Read, true)?;
        }
        // Two inputs that would write the same output file overwrite each other while both report
        // success, so the whole batch is refused here: before the folder is created, anything is
        // read, or the OCR engines load.
        let targets: Vec<std::path::PathBuf> = paths.iter().map(|p| child(&folder, &input_file_name(p))).collect();
        let shared = shared_file_names(&targets);
        if !shared.is_empty() {
            return Err(ToolError::InvalidArgs(format!(
                "more than one input would write to the same output file, so each would replace the one before it. Colliding file names: {}. Give each input a different file name, or run them in separate batches with different folders.",
                shared.join(", ")
            )));
        }
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let ocr = pdfcraft_engine::ocr::engine().map_err(failed)?;
        let mut out = Vec::new();
        for p in paths {
            let name = input_file_name(p);
            let result =
                self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| crate::read_input_bytes(&src).map_err(|e| e.to_string())).and_then(
                    |bytes| pdfcraft_engine::ocr::recognize_file(&name, std::sync::Arc::new(bytes), None, settings.clone(), &ocr, |_, _| true),
                );
            out.push(match result {
                Ok(r) => {
                    let target = child(&folder, &name);
                    write_atomic(&target, &r.bytes)?;
                    let skipped: Vec<usize> = r.pages.iter().filter(|p| p.skipped.is_some()).map(|p| p.page + 1).collect();
                    json!({ "path": p, "output": target.to_string_lossy(), "words": r.words(), "skipped_pages": skipped })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "files": out }))
    }

    /// Recognizes text on the open document's `pages` (every page when absent) and adds it,
    /// reporting the total word count and, per page, why it was skipped or what it found.
    pub(crate) fn ocr_recognize(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let doc = self.doc(a)?;
        // Recognizing needs only permission to change the document; the words it found are
        // returned only when copying is allowed too.
        let (id, copy) = (doc.id, doc.allows_copying());
        let found = self.session.recognize_text(id, &pages, settings).map_err(failed)?;
        let list: Vec<Value> = found
            .iter()
            .map(|p| match &p.skipped {
                Some(why) => json!({ "page": p.page + 1, "skipped": why }),
                None if copy => json!({ "page": p.page + 1, "words": p.words.len(), "already_on_page": p.already_on_page, "text": p.text() }),
                None => json!({ "page": p.page + 1, "words": p.words.len(), "already_on_page": p.already_on_page }),
            })
            .collect();
        Ok(json!({ "words": found.iter().map(|p| p.words.len()).sum::<usize>(), "pages": list }))
    }

    /// Whether text recognition's models are installed, the directories searched for them (in
    /// order), and the languages it reads, each as a code and a name.
    pub(crate) fn ocr_status(&self) -> Result<Value> {
        use pdfcraft_engine::ocr;
        let dirs: Vec<String> = ocr::Models::search_dirs().iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let langs: Vec<Value> = ocr::LANGUAGES.iter().map(|(c, n)| json!({ "code": c, "name": n })).collect();
        Ok(json!({ "available": ocr::available(), "search_dirs": dirs, "languages": langs }))
    }
}

/// The file name an input is written under: its own file name, or `document.pdf` when it has none.
fn input_file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into())
}

/// The output file names that two or more of `targets` share, each spelling listed once, in input
/// order. Names are compared without regard to case on Windows and macOS and exactly elsewhere; on
/// Windows a trailing dot or space is not part of the name (`scan.pdf.` is `scan.pdf`). Unicode
/// normalization forms (NFC and NFD spellings of one name) are not compared, so such inputs are
/// not caught. The folder is the same for every target, so only the names are compared.
fn shared_file_names(targets: &[std::path::PathBuf]) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<String, (usize, Vec<String>)> = std::collections::BTreeMap::new();
    for target in targets {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let key = if cfg!(windows) {
            name.trim_end_matches(['.', ' ']).to_lowercase()
        } else if cfg!(target_os = "macos") {
            // Case only; NFC and NFD spellings of one name stay distinct here (a known limit).
            name.to_lowercase()
        } else {
            name.clone()
        };
        let (count, spellings) = groups.entry(key).or_insert_with(|| (0, Vec::new()));
        *count += 1;
        if !spellings.contains(&name) {
            spellings.push(name);
        }
    }
    groups.into_values().filter(|(count, _)| *count > 1).flat_map(|(_, spellings)| spellings).collect()
}

#[cfg(test)]
mod shared_output_name_tests {
    use super::*;

    #[test]
    fn shared_output_names_follow_the_default_filesystem() {
        let targets = |names: &[&str]| -> Vec<std::path::PathBuf> { names.iter().map(|n| std::path::Path::new("out").join(n)).collect() };
        // A spelling that repeats is one shared name, listed once.
        assert_eq!(shared_file_names(&targets(&["a.pdf", "b.pdf", "a.pdf"])), vec!["a.pdf".to_string()]);
        assert!(shared_file_names(&targets(&["a.pdf", "b.pdf"])).is_empty());
        // Case is ignored on Windows and macOS only; a trailing dot is ignored on Windows only.
        let case = shared_file_names(&targets(&["scan.pdf", "Scan.PDF"]));
        let dot = shared_file_names(&targets(&["scan.pdf", "scan.pdf."]));
        assert_eq!(case.len(), if cfg!(any(windows, target_os = "macos")) { 2 } else { 0 }, "{case:?}");
        assert_eq!(dot.len(), if cfg!(windows) { 2 } else { 0 }, "{dot:?}");
    }
}
