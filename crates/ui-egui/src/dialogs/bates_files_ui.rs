//! Bates numbering ▸ Add to files…: numbers several files as one run, in list order, each file
//! starting after the last number of the one before. It writes numbered copies and never changes
//! the originals. Nothing is written unless every file can be numbered, and no copy replaces a
//! file. The numbering rules are the engine's ([`pdfcraft_engine::BatesFormat`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::host::{PickRequest, WriteIntent};
use craft::theme::variant::Tone;
use craft::ui::contract::{Bind, Show as _};
use craft::ui::dialog::dialog_id;
use craft::ui::display::{Shade, Text};
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind};
use pdfcraft_engine::{BATES_MAX_BYTES, BATES_MAX_FILES, BatesError, BatesFormat, EditError, Session};
use pdfcraft_render::OpenError;

use crate::PdfCraftApp;

/// The dialog's key.
const KEY: &str = "edit.bates_files";
/// The header and footer boxes, in [`BatesFormat::position`] order.
const POSITIONS: [&str; 6] = ["Top left", "Top center", "Top right", "Bottom left", "Bottom center", "Bottom right"];
/// Characters a copy name's ending can't hold on any desktop system.
const NOT_IN_NAMES: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// What the dialog's fields ask for.
struct Request {
    files: Vec<PathBuf>,
    format: BatesFormat,
    ending: String,
    /// Whether the copies go in a folder picked after OK, instead of beside each original.
    to_folder: bool,
}

impl Request {
    fn read(fields: &DialogFields) -> Self {
        let files = fields
            .list("files")
            .map(|items| items.iter().filter_map(|item| if let FieldValue::Text(path) = item { Some(PathBuf::from(path)) } else { None }).collect())
            .unwrap_or_default();
        let int = |key, default| fields.int(key).unwrap_or(default);
        let format = BatesFormat {
            start: u64::try_from(int("start", 1)).unwrap_or(0),
            digits: u8::try_from(int("digits", 6)).unwrap_or(0),
            prefix: fields.text("prefix").unwrap_or_default().to_owned(),
            suffix: fields.text("suffix").unwrap_or_default().to_owned(),
            position: fields.text("position").and_then(|p| p.parse().ok()).unwrap_or(5),
        };
        let ending = fields.text("ending").unwrap_or_default().to_owned();
        Request { files, format, ending, to_folder: fields.text("place") == Some("folder") }
    }

    /// Why OK can't run yet, if it can't.
    fn refusal(&self) -> Option<String> {
        if self.files.is_empty() {
            return Some(tl!("Add the files to number.").to_owned());
        }
        if self.files.len() > BATES_MAX_FILES {
            return Some(too_many());
        }
        if let Err(error) = self.format.check() {
            return Some(format_problem(&error));
        }
        if self.ending.contains(NOT_IN_NAMES) || self.ending.chars().any(char::is_control) {
            return Some(tl!("The name ending can't contain / \\ : * ? \" < > |").to_owned());
        }
        if self.ending.is_empty() && !self.to_folder {
            return Some(tl!("Copies beside the originals need a name ending, so they don't replace them.").to_owned());
        }
        None
    }
}

fn too_many() -> String {
    crate::i18n::fmt(tl!("A run numbers at most {n} files."), &[("n", &BATES_MAX_FILES.to_string())])
}

/// What is wrong with a format, in the UI language.
fn format_problem(error: &BatesError) -> String {
    match error {
        BatesError::Start => tl!("The start number must be at least 1.").to_owned(),
        BatesError::Digits => tl!("The number of digits must be from 1 to 12.").to_owned(),
        BatesError::Affix => tl!("The prefix and suffix can have at most 128 characters, and no # or >>.").to_owned(),
        BatesError::Position => tl!("Choose where the number goes.").to_owned(),
        BatesError::Count => too_many(),
        BatesError::NoPages(_) | BatesError::Overflow(_) => error.to_string(),
    }
}

/// The dialog over the values: OK waits for files and a format the Bates number can carry, and the
/// first number is shown as the page will.
fn dialog(fields: &mut DialogFields, open: Vec<PathBuf>) -> ParamDialog<'_> {
    let request = Request::read(fields);
    let refusal = request.refusal();
    let first = request.format.check().ok().map(|()| request.format.label(request.format.start));
    let positions = POSITIONS.iter().enumerate().map(|(i, name)| (i.to_string(), tl!(name).into())).collect();
    let places = vec![("beside".to_owned(), tl!("Beside each original").into()), ("folder".to_owned(), tl!("In a folder I choose").into())];
    let params = vec![
        Param::files("files", tl!("Files, numbered in this order")).open_files(open),
        Param::new("prefix", tl!("Prefix"), ParamKind::text("")),
        Param::new("start", tl!("Start number"), ParamKind::int(Some(1), None, 1)),
        Param::new("digits", tl!("Digits"), ParamKind::int(Some(1), Some(i64::from(pdfcraft_engine::BATES_MAX_DIGITS)), 6)),
        Param::new("suffix", tl!("Suffix"), ParamKind::text("")),
        Param::new("position", tl!("Position"), ParamKind::choice(positions, Some("5".into()))),
        Param::new("ending", tl!("Copy name ending"), ParamKind::text("_Bates")),
        Param::new("place", tl!("Save copies"), ParamKind::choice(places, Some("beside".into()))),
    ];
    let shown = refusal.clone();
    let mut dialog = ParamDialog::new(KEY, tl!("Add Bates Numbering to Files"), params)
        .values(Bind::bound(fields))
        .ok_label(tl!("Number").into())
        .ok_enabled(refusal.is_none())
        .below(move |ui, _| {
            if let Some(line) = &shown {
                Text::new(line.clone()).tone(Tone::Bad).show(ui);
            } else if let Some(first) = &first {
                Text::new(crate::i18n::fmt(tl!("The first page is numbered {label}."), &[("label", first)])).shade(Shade::Muted).show(ui);
            }
            Text::new(tl!("Numbered copies are written; the original files are not changed.")).show(ui);
        });
    if let Some(reason) = refusal {
        dialog = dialog.ok_reason(reason.into());
    }
    dialog
}

/// A file numbered and ready to write.
struct Numbered {
    target: PathBuf,
    bytes: Arc<Vec<u8>>,
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Where each copy goes: `<stem><ending>.pdf` beside its original, or in `folder`.
fn targets(files: &[PathBuf], ending: &str, folder: Option<&Path>) -> Vec<PathBuf> {
    files
        .iter()
        .map(|source| {
            let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let dir = folder.map_or_else(|| source.parent().map(Path::to_path_buf).unwrap_or_default(), Path::to_path_buf);
            dir.join(format!("{stem}{ending}.pdf"))
        })
        .collect()
}

/// A path as compared for "the same file": letter case is ignored, as on Windows and macOS.
fn same_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl PdfCraftApp {
    /// Bates numbering ▸ Add to files…: the dialog, offering the open documents' files.
    pub(crate) fn open_bates_files(&mut self, ctx: &egui::Context) {
        let open: Vec<PathBuf> = self.session.docs().iter().filter_map(|d| d.path.as_ref().map(PathBuf::from)).collect();
        let dialog = HostedParamDialog::new(ctx, KEY, DialogFields::new(), move |fields| dialog(fields, open.clone()));
        crate::dialogs::host::open(self, dialog);
    }

    /// Takes the dialog's answer: Number writes the copies, after asking for the folder when they
    /// go in one.
    pub(crate) fn bates_files_pass(&mut self, _ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        let request = Request::read(&result.fields);
        if let Some(reason) = request.refusal() {
            self.notify(reason);
            return;
        }
        if request.to_folder {
            let pick = PickRequest::folder("edit.bates_files").title(tl!("Choose a folder for the numbered copies"));
            self.pick_file(pick, None, move |app, _, picked| {
                if let Some(dir) = picked.first_path() {
                    app.number_files(&request, Some(&dir));
                }
            });
        } else {
            self.number_files(&request, None);
        }
    }

    /// Numbers `request`'s files and writes the copies, or writes nothing and says why.
    fn number_files(&mut self, request: &Request, folder: Option<&Path>) {
        let targets = targets(&request.files, &request.ending, folder);
        let numbered = self.refuse_targets(&request.files, &targets).and_then(|()| self.number_all(request, targets));
        let (numbered, first, last) = match numbered {
            Ok(done) => done,
            Err(reason) => {
                self.notify(crate::i18n::fmt(tl!("Nothing was numbered: {reason}"), &[("reason", &reason)]));
                return;
            }
        };
        for (done, file) in numbered.iter().enumerate() {
            if let Err(e) = self.host.files().write(&file.target, &file.bytes, WriteIntent::SaveAs) {
                let (n, total) = (done.to_string(), numbered.len().to_string());
                let args = [("n", n.as_str()), ("total", total.as_str()), ("name", &name_of(&file.target)), ("e", &e.to_string())];
                self.notify_fmt("Wrote {n} of {total} numbered copies; couldn't write {name}: {e}", &args);
                return;
            }
        }
        let n = numbered.len().to_string();
        let next = last.checked_add(1).map_or_else(|| tl!("none").to_owned(), |next| next.to_string());
        let (first, last) = (request.format.label(first), request.format.label(last));
        let args = [("n", n.as_str()), ("first", &first), ("last", &last), ("next", &next)];
        // The next number is what a following run starts from, so the series can go on.
        self.notify_fmt("Numbered {n} files from {first} to {last}. The next start number is {next}.", &args);
    }

    /// Refuses a copy that would land on an original, on another copy, or on a file that exists.
    fn refuse_targets(&self, files: &[PathBuf], targets: &[PathBuf]) -> Result<(), String> {
        let sources: HashSet<String> = files.iter().map(|p| same_key(p)).collect();
        let mut taken = HashSet::new();
        for target in targets {
            let name = name_of(target);
            if sources.contains(&same_key(target)) {
                return Err(crate::i18n::fmt(tl!("the copy {name} would replace an original"), &[("name", &name)]));
            }
            if !taken.insert(same_key(target)) {
                return Err(crate::i18n::fmt(tl!("two copies would be named {name}"), &[("name", &name)]));
            }
            if self.host.files().exists(target) {
                return Err(crate::i18n::fmt(tl!("{name} already exists"), &[("name", &name)]));
            }
        }
        Ok(())
    }

    /// Reads and numbers every file in a session of its own, so no tab opens. Gives the copies and
    /// the run's first and last numbers.
    fn number_all(&self, request: &Request, targets: Vec<PathBuf>) -> Result<(Vec<Numbered>, u64, u64), String> {
        let mut session = Session::new();
        let mut ids = Vec::with_capacity(request.files.len());
        let mut read = 0u64;
        for source in &request.files {
            ids.push(self.open_source(&mut session, source, &mut read)?);
        }
        let pages: Vec<usize> = ids.iter().map(|&id| session.get(id).map_or(0, |d| d.info.pages.len())).collect();
        let name = |index: usize| request.files.get(index).map(|p| name_of(p)).unwrap_or_default();
        let spans = request.format.spans(&pages).map_err(|error| match error {
            BatesError::NoPages(i) => crate::i18n::fmt(tl!("{name} has no pages"), &[("name", &name(i))]),
            BatesError::Overflow(i) => crate::i18n::fmt(tl!("the numbers run out at {name}"), &[("name", &name(i))]),
            other => other.to_string(),
        })?;
        let mut numbered = Vec::with_capacity(ids.len());
        for (index, ((&id, span), target)) in ids.iter().zip(&spans).zip(targets).enumerate() {
            let failed = |e: EditError| crate::i18n::fmt(tl!("couldn't number {name}: {e}"), &[("name", &name(index)), ("e", &e.to_string())]);
            session.add_bates(id, &request.format, span.first).map_err(failed)?;
            let bytes = session.save_full_bytes(id).map_err(failed)?;
            session.check_save(id, &bytes).map_err(failed)?;
            numbered.push(Numbered { target, bytes });
        }
        let first = spans.first().map_or(request.format.start, |s| s.first);
        let last = spans.last().map_or(first, |s| s.last);
        Ok((numbered, first, last))
    }

    /// Opens `source` in `session`, refusing a file that can't be read, needs a password or is
    /// signed (numbering would break its signature).
    fn open_source(&self, session: &mut Session, source: &Path, read: &mut u64) -> Result<pdfcraft_engine::DocId, String> {
        let name = name_of(source);
        let bytes = self
            .host
            .files()
            .read(source)
            .map_err(|e| crate::i18n::fmt(tl!("couldn't read {name}: {e}"), &[("name", &name), ("e", &e.to_string())]))?;
        // The sources, their documents and the copies are all held until the copies are written.
        *read = u64::try_from(bytes.len()).ok().and_then(|n| read.checked_add(n)).filter(|total| *total <= BATES_MAX_BYTES).ok_or_else(|| {
            let mb = (BATES_MAX_BYTES / (1024 * 1024)).to_string();
            crate::i18n::fmt(tl!("the files come to more than {n} MB at {name}"), &[("n", &mb), ("name", &name)])
        })?;
        let id = session.open(name.clone(), None, Arc::new(bytes), None).map_err(|e| match e {
            OpenError::NeedsPassword | OpenError::WrongPassword => crate::i18n::fmt(tl!("{name} is password-protected"), &[("name", &name)]),
            other => crate::i18n::fmt(tl!("couldn't open {name}: {e}"), &[("name", &name), ("e", &other.to_string())]),
        })?;
        if session.get(id).is_some_and(pdfcraft_engine::Document::is_signed) {
            return Err(crate::i18n::fmt(tl!("{name} is signed, and numbering it would break the signature"), &[("name", &name)]));
        }
        Ok(id)
    }
}
