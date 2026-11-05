//! Scan & OCR ▸ Recognize text in multiple files: PDFs and whole folders (with their subfolders,
//! if asked) are read into an output folder, the folder structure kept, and the originals never
//! changed. Finding the files and writing the results is `pdfcraft_engine::batch`, the same as
//! the `doc_batch` tool and `pdfcraft-cli batch`, so a name the output folder already has is kept
//! and the new file gets a number. The desktop build only: the web build has no folders, and its
//! Recognize text in multiple files asks for the PDFs instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Align, Layout, RichText};
use pdfcraft_engine::batch::{self, Discovery, Recipe, StepLog};
use pdfcraft_engine::ocr::OcrSettings;

use crate::ocr_ui::{BatchProgress, unstarted};
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The most inputs the list takes (the engine reads at most 10 000 files and folders anyway).
const MAX_INPUTS: usize = 1000;
/// The most failed files the summary names; the rest are counted.
const MAX_FAILED_SHOWN: usize = 5;

/// What to read and where to write it. The recognition settings are the Recognize Text dialog's
/// ([`crate::ocr_ui::OcrDraft`]), shared with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrBatchDraft {
    /// Files and folders, in the order they were added, each once.
    pub inputs: Vec<PathBuf>,
    /// Where the searchable copies go.
    pub out_dir: Option<PathBuf>,
    /// Read the folders' subfolders too.
    pub recursive: bool,
    /// What the file system said about `inputs` and `out_dir`, for drawing: the dialog is drawn
    /// every frame, and a folder on an unresponsive network drive must not stall each one.
    checked: Option<Checked>,
}

/// [`OcrBatchDraft::refresh`]'s answers, with the inputs and output folder they are for.
#[derive(Clone, Debug, PartialEq)]
struct Checked {
    inputs: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
    /// Whether each input is a folder, in the order of `inputs`.
    folders: Vec<bool>,
    problem: Option<&'static str>,
}

impl OcrBatchDraft {
    /// Add `paths` that aren't in the list yet.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if self.inputs.len() < MAX_INPUTS && !self.inputs.contains(&path) {
                self.inputs.push(path);
            }
        }
    }

    /// Why the job can't start, to show under the output folder; `None` when it can, or when
    /// what's missing (no input, no output folder) is plain from the empty fields.
    /// Asks the file system each time: [`Self::shown_problem`] is what the dialog draws.
    pub fn problem(&self) -> Option<&'static str> {
        let folders: Vec<bool> = self.inputs.iter().map(|p| p.is_dir()).collect();
        self.problem_with(&folders)
    }

    /// [`Self::problem`] with whether each input is a folder already known.
    fn problem_with(&self, folders: &[bool]) -> Option<&'static str> {
        let out = self.out_dir.as_deref()?;
        let out = batch::absolute(out).ok()?;
        // The engine's rule: the results must not land in a folder that is being read.
        let reads_output = self
            .inputs
            .iter()
            .zip(folders)
            .filter(|(_, folder)| **folder)
            .filter_map(|(p, _)| batch::absolute(p).ok())
            .any(|input| input.starts_with(&out));
        reads_output.then_some("The output folder can't be one of the folders being read, or contain one.")
    }

    /// Whether Recognize text can start, asking the file system: checked again when it starts.
    pub fn ready(&self) -> bool {
        !self.inputs.is_empty() && self.out_dir.is_some() && self.problem().is_none()
    }

    /// Check the inputs and the output folder again, only if they changed since the last check
    /// (also when written directly rather than through [`Self::add`]).
    pub fn refresh(&mut self) {
        if self.checked.as_ref().is_some_and(|c| c.inputs == self.inputs && c.out_dir == self.out_dir) {
            return;
        }
        let folders: Vec<bool> = self.inputs.iter().map(|p| p.is_dir()).collect();
        let problem = self.problem_with(&folders);
        self.checked = Some(Checked { inputs: self.inputs.clone(), out_dir: self.out_dir.clone(), folders, problem });
    }

    /// [`Self::problem`] as of the last [`Self::refresh`].
    pub fn shown_problem(&self) -> Option<&'static str> {
        self.checked.as_ref().and_then(|c| c.problem)
    }

    /// Whether input `i` was a folder at the last [`Self::refresh`].
    fn shown_folder(&self, i: usize) -> bool {
        self.checked.as_ref().and_then(|c| c.folders.get(i)).copied().unwrap_or(false)
    }

    /// [`Self::ready`] as of the last [`Self::refresh`]: what the Recognize text button shows.
    fn shown_ready(&self) -> bool {
        !self.inputs.is_empty() && self.out_dir.is_some() && self.checked.is_some() && self.shown_problem().is_none()
    }
}

/// The dialog. Returns (start, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let title = crate::i18n::menu_label("ocr.recognize_batch", "Recognize text in multiple files…");
    ui.label(RichText::new(title.trim_end_matches('…').trim_end_matches("...")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    app.ocr_batch_draft.refresh();
    let available = pdfcraft_engine::ocr::available();
    card(ui, t, None, |ui| inputs(ui, app, t));
    card(ui, t, Some(tl!("Output folder")), |ui| output(ui, app, t));
    card(ui, t, Some(tl!("Settings")), |ui| crate::ocr_ui::settings_fields(ui, &mut app.ocr_draft));
    if !available {
        hint(
            ui,
            t,
            tl!(
                "Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them."
            ),
        );
        ui.add_space(6.0);
    }
    let ready = available && app.ocr_batch_draft.shown_ready() && app.ocr_batch.is_none();
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Recognize text"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// A group of settings in a card, with its name above it when it has one.
fn card<R>(ui: &mut egui::Ui, t: &Tokens, title: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if let Some(title) = title {
        ui.label(RichText::new(title).font(theme::semibold(13.0)));
    }
    let inner = egui::Frame::new()
        .fill(t.hover)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(8.0);
    inner
}

/// An explanation: small, muted, wrapping.
fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(t.text_muted)).wrap());
}

/// The files and folders to read, with Add files…, Add folder… and Include subfolders.
fn inputs(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let draft = &mut app.ocr_batch_draft;
    if draft.inputs.is_empty() {
        hint(ui, t, tl!("Add files or folders to recognize."));
    } else {
        let mut remove = None;
        egui::ScrollArea::vertical().id_salt("ocr-batch-inputs").max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, path) in draft.inputs.iter().enumerate() {
                ui.horizontal(|ui| {
                    let icon = if draft.shown_folder(i) { "folder" } else { "file-text" };
                    ui.add(crate::icons::image(icon, 16.0, t.icon));
                    path_label(ui, path, t, 30.0);
                    if crate::icons::button(ui, "x", 22.0, false, tl!("Remove")).clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove
            && i < draft.inputs.len()
        {
            draft.inputs.remove(i);
        }
    }
    ui.horizontal_wrapped(|ui| {
        if widgets::pill_button(ui, tl!("Add files…"), false).clicked() {
            app.ocr_batch_add_files();
        }
        if widgets::pill_button(ui, tl!("Add folder…"), false).clicked() {
            app.ocr_batch_add_folder();
        }
    });
    ui.add(widgets::checkbox(&mut app.ocr_batch_draft.recursive, tl!("Include subfolders")));
}

/// A path, left to right in every language, in the room left after `reserve` points (for the
/// buttons after it), whole in its tooltip. A path too long for it loses its start, not its end:
/// the folder or file name is what tells the entries apart.
fn path_label(ui: &mut egui::Ui, path: &Path, t: &Tokens, reserve: f32) {
    let room = (ui.available_width() - reserve).max(0.0);
    let text = path.to_string_lossy();
    let font = theme::regular(12.5);
    let width = |s: &str| crate::bidi::layout_ltr(ui, s, font.clone(), t.text).size().x;
    let shown = if width(&text) <= room { text.to_string() } else { tail_that_fits(&text, |tail| width(&format!("…{tail}")) <= room) };
    ui.allocate_ui(egui::vec2(room, 22.0), |ui| {
        ui.set_width(room);
        widgets::label_ltr_truncated(ui, shown, font.clone(), t.text).on_hover_text(text.as_ref());
    });
}

/// "…" and the longest end of `text` that `fits`, cut at a character boundary: a binary search
/// over where the end starts, so a long path is measured a few times, not once per character.
fn tail_that_fits(text: &str, fits: impl Fn(&str) -> bool) -> String {
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, starts.len());
    // The first start whose tail fits; the empty tail (hi) always counts as fitting.
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let tail = starts.get(mid).and_then(|&i| text.get(i..)).unwrap_or("");
        if fits(tail) { hi = mid } else { lo = mid + 1 }
    }
    format!("…{}", starts.get(lo).and_then(|&i| text.get(i..)).unwrap_or(""))
}

/// The output folder, Browse…, and what happens to existing files.
fn output(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.add(crate::icons::image("folder", 16.0, t.icon));
        match app.ocr_batch_draft.out_dir.clone() {
            Some(dir) => path_label(ui, &dir, t, 136.0),
            None => {
                let room = (ui.available_width() - 136.0).max(0.0);
                ui.allocate_ui(egui::vec2(room, 22.0), |ui| {
                    ui.set_width(room);
                    ui.add(egui::Label::new(RichText::new(tl!("Choose a folder for the searchable files")).color(t.text_muted)).truncate());
                });
            }
        }
        if widgets::pill_button(ui, tl!("Browse…"), false).clicked() {
            app.ocr_batch_choose_output();
        }
    });
    if let Some(problem) = app.ocr_batch_draft.shown_problem() {
        ui.add(egui::Label::new(RichText::new(tl!(problem)).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap());
    }
    hint(ui, t, tl!("The originals are not changed. A file of the same name in the output folder is kept, and the new one gets a number."));
}

impl PdfCraftApp {
    /// Add PDFs to the list.
    fn ocr_batch_add_files(&mut self) {
        let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Add files…").to_string()).add_filter("PDF", &["pdf", "PDF"]);
        self.ask(crate::pickers::Ask::Files(dialog), None, |app, paths| app.ocr_batch_draft.add(paths));
    }

    /// Add a folder to the list.
    fn ocr_batch_add_folder(&mut self) {
        let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Add folder…").to_string());
        self.ask_one(crate::pickers::Ask::Folder(dialog), None, |app, dir| app.ocr_batch_draft.add([dir]));
    }

    /// Choose where the searchable copies go.
    fn ocr_batch_choose_output(&mut self) {
        let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose a folder for the searchable files").to_string());
        self.ask_one(crate::pickers::Ask::Folder(dialog), None, |app, dir| app.ocr_batch_draft.out_dir = Some(dir));
    }

    /// Recognize text in the files and folders of the dialog, in the background, with the
    /// progress and summary of Recognize text in multiple files.
    pub fn start_ocr_batch(&mut self) {
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        let draft = self.ocr_batch_draft.clone();
        let Some(out) = draft.out_dir.clone().filter(|_| draft.ready()) else { return };
        let settings = self.ocr_draft.settings();
        let progress = Arc::new(Mutex::new(BatchProgress::default()));
        let p = progress.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            // Each file is guarded in `run`; this catches a panic outside them (#3919).
            let summary = pdfcraft_engine::guard(|| run(&draft.inputs, &out, draft.recursive, settings, &p)).unwrap_or_else(|panic| failed(&panic));
            if let Ok(mut s) = p.lock() {
                s.done = s.total;
                s.message = Some(summary);
            }
        };
        if self.run_inline {
            work();
        } else {
            unstarted(&progress, std::thread::Builder::new().name("pdfcraft-ocr-batch".into()).spawn(work), |s, e| s.message = Some(e));
        }
        self.ocr_batch = Some(progress);
        self.poll_ocr();
    }
}

/// What reading one file gives: the result to write and what was done to it.
type Processed = Result<(Arc<Vec<u8>>, Vec<StepLog>), String>;

/// Find the files, recognize text in each and publish it under `out`; returns the summary.
fn run(inputs: &[PathBuf], out: &Path, recursive: bool, settings: OcrSettings, p: &Mutex<BatchProgress>) -> String {
    let recipe = Recipe::ocr(settings);
    run_with(inputs, out, recursive, &recipe, pdfcraft_engine::ocr::engine().map(|_| ()), p, |path| {
        recipe.process(path, |page, pages| BatchProgress::page(p, page, pages))
    })
}

/// [`run`] with the models' state and the reading of one file passed in (tests).
fn run_with(
    inputs: &[PathBuf],
    out: &Path,
    recursive: bool,
    recipe: &Recipe,
    models: Result<(), String>,
    p: &Mutex<BatchProgress>,
    mut process: impl FnMut(&Path) -> Processed,
) -> String {
    let found = (|| {
        let out = batch::absolute(out)?;
        let roots = inputs.iter().map(|path| batch::absolute(path)).collect::<Result<Vec<_>, _>>()?;
        Discovery::find(&roots, &out, recursive).map(|found| (out, found))
    })();
    let (out, found) = match found {
        Ok(found) => found,
        Err(e) => return failed(&e),
    };
    let mut skipped = found.skipped.len();
    let files: Vec<(PathBuf, PathBuf)> = found
        .files
        .into_iter()
        .filter(|(path, _)| {
            let ok = recipe.accepts(path);
            skipped += usize::from(!ok);
            ok
        })
        .collect();
    if files.is_empty() {
        return tl!("No PDF files were found.").to_string();
    }
    if let Ok(mut s) = p.lock() {
        s.total = files.len();
    }
    // The models load once; missing ones are said once, rather than per file.
    if let Err(e) = models {
        return failed(&e);
    }
    let (mut ok, mut words, mut errors, mut cancelled) = (0usize, 0usize, Vec::new(), false);
    for (i, (path, relative)) in files.into_iter().enumerate() {
        match p.lock() {
            Ok(s) if s.cancel => {
                cancelled = true;
                break;
            }
            Ok(mut s) => s.done = i,
            Err(_) => {}
        }
        // A file that panics fails alone; the others are still read and counted.
        let result = pdfcraft_engine::guard(|| {
            batch::unchanged(&path)?;
            let (bytes, log) = process(&path)?;
            batch::publish(&batch::confined_output(&out, &relative)?, &bytes)?;
            Ok(log.iter().map(|l| if let StepLog::Ocr { words, .. } = l { *words } else { 0 }).sum::<usize>())
        })
        .unwrap_or_else(Err);
        match result {
            // Stopped partway: nothing of this file was written.
            Err(e) if e == batch::CANCELLED => {
                cancelled = true;
                break;
            }
            Ok(n) => {
                ok += 1;
                words = words.saturating_add(n);
            }
            Err(e) => errors.push(format!("{}: {e}", relative.display())),
        }
        BatchProgress::file_done(p);
    }
    let summary = summary(ok, words, skipped, &errors);
    if cancelled { crate::i18n::fmt(tl!("Text recognition cancelled. {summary}"), &[("summary", &summary)]) } else { summary }
}

/// "Recognized 120 words in 3 files; skipped: 2; failed: a.pdf: …", in the current language.
fn summary(ok: usize, words: usize, skipped: usize, errors: &[String]) -> String {
    let mut msg = if ok == 1 {
        crate::i18n::fmt(tl!("Recognized {w} words in 1 file"), &[("w", &words.to_string())])
    } else {
        crate::i18n::fmt(tl!("Recognized {w} words in {n} files"), &[("w", &words.to_string()), ("n", &ok.to_string())])
    };
    if skipped > 0 {
        msg.push_str(&crate::i18n::fmt(tl!("; skipped: {n}"), &[("n", &skipped.to_string())]));
    }
    if !errors.is_empty() {
        let mut list = errors.iter().take(MAX_FAILED_SHOWN).cloned().collect::<Vec<_>>().join("; ");
        if errors.len() > MAX_FAILED_SHOWN {
            list.push_str(&format!("; … (+{})", errors.len() - MAX_FAILED_SHOWN));
        }
        msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &list)]));
    }
    msg
}

/// "Recognize text failed: {e}", in the current language.
fn failed(e: &str) -> String {
    crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!("Recognize text")), ("e", e)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_path_keeps_its_end() {
        let fits = |n: usize| move |s: &str| s.chars().count() <= n;
        assert_eq!(tail_that_fits("/home/me/Scans/2025", fits(9)), "…cans/2025");
        assert_eq!(tail_that_fits("/home/me/Scans/2025", fits(0)), "…");
        assert_eq!(tail_that_fits("/häuser/größe", fits(5)), "…größe", "cut between characters, not bytes");
        assert_eq!(tail_that_fits("", fits(3)), "…");
    }

    #[test]
    fn the_list_keeps_each_path_once() {
        let mut d = OcrBatchDraft::default();
        d.add([PathBuf::from("a.pdf"), PathBuf::from("scans"), PathBuf::from("a.pdf")]);
        assert_eq!(d.inputs, vec![PathBuf::from("a.pdf"), PathBuf::from("scans")]);
    }

    #[test]
    fn an_output_folder_that_is_or_holds_an_input_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let scans = dir.path().join("scans");
        std::fs::create_dir_all(scans.join("sub")).unwrap();
        let mut d = OcrBatchDraft { inputs: vec![scans.clone()], recursive: true, ..Default::default() };
        assert!(!d.ready(), "no output folder yet");
        for bad in [scans.clone(), dir.path().to_path_buf()] {
            d.out_dir = Some(bad);
            assert!(d.problem().is_some() && !d.ready());
        }
        // Inside an input folder is fine: the engine leaves it out of what it reads.
        d.out_dir = Some(scans.join("ocr"));
        assert!(d.problem().is_none() && d.ready());
        d.out_dir = Some(dir.path().join("out"));
        assert!(d.ready());
    }

    #[test]
    fn the_summary_counts_skipped_files_and_names_only_the_first_failures() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        assert_eq!(summary(2, 40, 0, &[]), "Recognized 40 words in 2 files");
        let errors: Vec<String> = (1..=7).map(|i| format!("{i}.pdf: broken")).collect();
        let s = summary(1, 3, 2, &errors);
        assert!(s.starts_with("Recognized 3 words in 1 file; skipped: 2; failed: 1.pdf: broken"), "{s}");
        assert!(s.ends_with("5.pdf: broken; … (+2)"), "{s}");
    }

    /// The dialog draws what the last check found: it doesn't ask the file system again until
    /// the inputs or the output folder change.
    #[test]
    fn drawing_does_not_ask_the_file_system_again() {
        let dir = tempfile::tempdir().unwrap();
        let scans = dir.path().join("scans");
        std::fs::create_dir_all(&scans).unwrap();
        let mut d = OcrBatchDraft { inputs: vec![scans.clone()], out_dir: Some(scans.clone()), ..Default::default() };
        assert!(d.shown_problem().is_none() && !d.shown_ready(), "nothing checked yet");
        d.refresh();
        assert!(d.shown_problem().is_some() && d.shown_folder(0));
        std::fs::remove_dir(&scans).unwrap();
        d.refresh();
        assert!(d.problem().is_none(), "the file system has changed");
        assert!(d.shown_problem().is_some() && d.shown_folder(0), "not checked again for the same paths");
        // A change written straight into the field is checked on the next refresh.
        d.out_dir = Some(dir.path().join("out"));
        d.refresh();
        assert!(d.shown_problem().is_none() && !d.shown_folder(0) && d.shown_ready());
    }

    /// A worker that can't start ends the run with the reason, so the progress card goes and
    /// the next run isn't refused as already running.
    #[test]
    fn a_worker_that_cannot_start_ends_the_run() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let p = Mutex::new(BatchProgress::default());
        unstarted(&p, Ok(()), |s: &mut BatchProgress, e| s.message = Some(e));
        assert!(p.lock().unwrap().message.is_none());
        unstarted::<_, ()>(&p, Err(std::io::Error::other("no threads left")), |s, e| s.message = Some(e));
        assert_eq!(p.lock().unwrap().message.as_deref(), Some("Recognize text failed: no threads left"));
    }

    /// Cancelled before a file is read: nothing is written, and the summary says it was cancelled.
    #[test]
    fn a_cancelled_batch_writes_nothing_more() {
        if pdfcraft_engine::ocr::engine().is_err() {
            eprintln!("skipped: OCR models not installed");
            return;
        }
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let dir = tempfile::tempdir().unwrap();
        let scans = dir.path().join("scans");
        std::fs::create_dir_all(&scans).unwrap();
        let pdf = pdfcraft_engine::Session::new().create_blank(200.0, 300.0, 1).unwrap();
        std::fs::write(scans.join("a.pdf"), &*pdf).unwrap();
        let out = dir.path().join("out");
        let p = Mutex::new(BatchProgress { cancel: true, ..Default::default() });
        let summary = run(&[scans], &out, false, OcrSettings::default(), &p);
        assert!(summary.starts_with("Text recognition cancelled. Recognized 0 words in 0 files"), "{summary}");
        assert!(!out.join("a.pdf").exists());
    }

    /// A file that panics fails alone: the files after it are still read, and the summary
    /// counts them and names the one that failed.
    #[test]
    fn a_file_that_panics_fails_alone() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let dir = tempfile::tempdir().unwrap();
        let scans = dir.path().join("scans");
        std::fs::create_dir_all(&scans).unwrap();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            std::fs::write(scans.join(name), b"%PDF").unwrap();
        }
        let out = dir.path().join("out");
        let p = Mutex::new(BatchProgress::default());
        let recipe = Recipe::ocr(OcrSettings::default());
        let summary = run_with(&[scans], &out, false, &recipe, Ok(()), &p, |path| {
            if path.ends_with("b.pdf") {
                panic!("broken scan");
            }
            Ok((Arc::new(b"%PDF result".to_vec()), vec![StepLog::Ocr { words: 2, skipped_pages: 0 }]))
        });
        assert_eq!(summary, "Recognized 4 words in 2 files; failed: b.pdf: broken scan");
        assert!(out.join("a.pdf").exists() && out.join("c.pdf").exists() && !out.join("b.pdf").exists());
    }

    /// Links that turn up while the batch runs are not followed: an output subfolder that links
    /// elsewhere gets no result, and a file swapped for a link after it was found is not read.
    #[cfg(unix)]
    #[test]
    fn links_made_during_the_batch_are_not_followed() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let (scans, out, elsewhere) = (base.join("scans"), base.join("out"), base.join("elsewhere"));
        std::fs::create_dir_all(scans.join("sub")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(scans.join("a.pdf"), b"%PDF").unwrap();
        std::fs::write(scans.join("b.pdf"), b"%PDF").unwrap();
        std::fs::write(scans.join("sub/c.pdf"), b"%PDF").unwrap();
        std::fs::write(elsewhere.join("secret.pdf"), b"%PDF secret").unwrap();
        std::os::unix::fs::symlink(&elsewhere, out.join("sub")).unwrap();
        let p = Mutex::new(BatchProgress::default());
        let recipe = Recipe::ocr(OcrSettings::default());
        let mut read = Vec::new();
        let summary = run_with(std::slice::from_ref(&scans), &out, true, &recipe, Ok(()), &p, |path| {
            read.push(path.to_path_buf());
            if path.ends_with("a.pdf") {
                // b.pdf, found already, becomes a link before its turn.
                std::fs::remove_file(scans.join("b.pdf")).unwrap();
                std::os::unix::fs::symlink(elsewhere.join("secret.pdf"), scans.join("b.pdf")).unwrap();
            }
            Ok((Arc::new(b"%PDF result".to_vec()), Vec::new()))
        });
        assert_eq!(read, vec![scans.join("a.pdf"), scans.join("sub/c.pdf")], "b.pdf is not read");
        assert!(summary.starts_with("Recognized 0 words in 1 file; failed: b.pdf: "), "{summary}");
        assert!(summary.contains("sub/c.pdf: "), "{summary}");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 1, "nothing written through out/sub");
        assert!(out.join("a.pdf").exists());
    }
}
