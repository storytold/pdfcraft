use super::{Edit, PdfCraftApp, StagingName, staging_suffixes};

/// A save held back by [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged): the file
/// on disk is another version than the document came from (#2166).
#[cfg(not(target_arch = "wasm32"))]
pub struct PendingOverwrite {
    pub id: pdfcraft_engine::DocId,
    pub dest: String,
    target: SaveTarget,
    /// What was to run once the document is written (close its tab, quit).
    after: Box<dyn FnOnce(&mut PdfCraftApp) + Send>,
}

/// A save held back by [`Dialog::RedactBeforeSave`](crate::Dialog::RedactBeforeSave): the
/// document has redaction marks that aren't applied, so what they cover would be saved (#4741).
pub struct PendingRedactSave {
    pub id: pdfcraft_engine::DocId,
    target: SaveTarget,
    /// Where the Save As picker said to write, when marks were added while it was open; `None`
    /// goes on as Save does, asking where if it must.
    dest: Option<String>,
    /// What was to run once the document is written (close its tab, quit).
    after: Box<dyn FnOnce(&mut PdfCraftApp) + Send>,
}

/// Where Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// The file's own path; asks for one if the document has none (e.g. opened from bytes).
    InPlace,
    /// Always ask for a new location.
    As,
}

impl PdfCraftApp {
    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, target: SaveTarget) -> bool {
        match self.active {
            Some(i) => self.save_view(i, target),
            None => false,
        }
    }

    /// Save the document shown in tab `index`. Returns `true` if it was written now. When it has
    /// to ask where (a new document, or Save As) it returns `false` and saves on a later frame,
    /// once the user has chosen.
    pub fn save_view(&mut self, index: usize, target: SaveTarget) -> bool {
        self.save_then(index, target, |_| {})
    }

    /// [`Self::save_view`], then `after` once the document is written: now, or on a later frame
    /// when the user had to choose where. `after` never runs when the save fails or is cancelled.
    pub(crate) fn save_then(&mut self, index: usize, target: SaveTarget, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        self.save_then_checked(index, target, true, after)
    }

    /// [`Self::save_then`]; `check_marks` false skips the question about unapplied redaction
    /// marks, for the save the user just answered it for.
    fn save_then_checked(&mut self, index: usize, target: SaveTarget, check_marks: bool, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        // A redaction mark only outlines what it covers until it is applied: saved as it is, the
        // text and images under it stay in the file for anyone to read (#4741). Ask first, unless
        // these marks were already kept on purpose and nothing changed since.
        if check_marks
            && let Some(id) = self.views.get(index).map(|v| v.id)
            && self.must_ask_about_marks(id)
        {
            self.hold_for_marks(PendingRedactSave { id, target, dest: None, after: Box::new(after) });
            return false;
        }
        // Marks kept on purpose stay kept for what this save writes, although writing moves the
        // document's generation on.
        let kept = self
            .views
            .get(index)
            .and_then(|v| self.session.get(v.id).map(|d| (v.id, d.edit_generation())))
            .filter(|(id, generation)| self.kept_redaction_marks.get(id) == Some(generation))
            .map(|(id, _)| id);
        let after = move |app: &mut Self| {
            if let Some(id) = kept
                && let Some(generation) = app.session.get(id).map(|d| d.edit_generation())
            {
                app.kept_redaction_marks.insert(id, generation);
            }
            after(app);
        };
        // What's typed in a field is part of what's saved (#166).
        if let Some(id) = self.views.get(index).map(|v| v.id)
            && !self.commit_typing_in(id)
        {
            return false;
        }
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let (name, path) = (doc.name.clone(), doc.path.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let doc_path = path.clone();
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(p)) => p.clone(),
                // An email attachment's temporary copy is no place to keep work: ask, as for a new
                // document (`is_mail_temp_copy`).
                (SaveTarget::InPlace, Some(p), _) if !is_mail_temp_copy(&p) => p,
                _ => {
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
                    let dialog = in_document_folder(dialog, doc_path.as_deref());
                    let asked_at = self.session.get(id).map(|d| d.edit_generation());
                    // The bytes are taken once the user has chosen, so edits made meanwhile are saved too.
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, dest| {
                        let dest = dest.to_string_lossy().into_owned();
                        // The window takes input while the picker is open (on Windows it has no
                        // parent window): marks added meanwhile are asked about before anything
                        // is written there.
                        if app.session.get(id).map(|d| d.edit_generation()) != asked_at && app.must_ask_about_marks(id) {
                            app.hold_for_marks(PendingRedactSave { id, target, dest: Some(dest), after: Box::new(after) });
                            return;
                        }
                        app.save_doc_to_then(id, &dest, target, Box::new(after));
                    });
                    return false;
                }
            };
            self.save_doc_to_then(id, &destination, target, Box::new(after))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            if !self.flatten_fill_sign_for_save(id) {
                return false;
            }
            let bytes = match self.session.save_bytes(id) {
                Ok(b) => b,
                Err(e) => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return false;
                }
            };
            // An embedding page that asked for saves (`?host=parent`) gets the bytes; otherwise
            // the browser downloads them.
            let to_host = self.host_save.is_some();
            let delivered = match &self.host_save {
                Some(save) => save(&name, &bytes, matches!(target, SaveTarget::As)),
                None => download(&name, &bytes),
            };
            match delivered {
                Ok(()) => {
                    let _ = self.session.mark_saved(id, bytes, None);
                    if let Some(doc) = self.session.get(id)
                        && let Some(view) = self.views.get_mut(index)
                    {
                        view.document_changed(&doc.info);
                    }
                    // The host reports where the file went (and any failure) itself.
                    if !to_host {
                        self.notify_fmt("Downloaded {name}", &[("name", &name)]);
                    }
                    after(self);
                    true
                }
                Err(e) if to_host => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e)]);
                    false
                }
                Err(e) => {
                    self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    false
                }
            }
        }
    }

    /// [`Self::save_doc_to`], then `after`. When `dest` is the document's own file and it is
    /// another version on disk than the one the document came from (something else wrote it
    /// since the open or the last save, #2166), it asks first: the save waits in
    /// `pending_overwrite` behind [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged).
    /// Returns `true` if written now.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to_then(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget, after: Box<dyn FnOnce(&mut Self) + Send>) -> bool {
        let own = self.session.get(id).and_then(|d| d.path.as_deref()).is_some_and(|p| crate::same_file(p, dest));
        if own && self.watch.disk_differs(id, dest, self.session.get(id).map(|d| d.saved_bytes())) {
            self.pending_overwrite = Some(PendingOverwrite { id, dest: dest.to_string(), target, after });
            self.dialog = Some(crate::Dialog::OverwriteChanged);
            return false;
        }
        let saved = self.save_doc_to(id, dest, target);
        if saved {
            after(self);
        }
        saved
    }

    /// The dialog's Save anyway: write over the changed file.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn overwrite_changed_file(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if self.save_doc_to(p.id, &p.dest, p.target) {
            (p.after)(self);
        }
    }

    /// The dialog's Save as…: leave the file on disk as it is and ask where to put this version.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn save_changed_file_elsewhere(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if let Some(index) = self.views.iter().position(|v| v.id == p.id) {
            self.save_then(index, SaveTarget::As, p.after);
        }
    }

    /// Whether saving document `id` must first ask about its redaction marks: it has some, and
    /// they weren't kept on purpose at its current generation.
    fn must_ask_about_marks(&self, id: pdfcraft_engine::DocId) -> bool {
        self.session.get(id).is_some_and(|doc| doc.redaction_marks() > 0 && self.kept_redaction_marks.get(&id) != Some(&doc.edit_generation()))
    }

    /// Hold save `p` behind [`Dialog::RedactBeforeSave`](crate::Dialog::RedactBeforeSave).
    fn hold_for_marks(&mut self, p: PendingRedactSave) {
        self.pending_redact_save = Some(p);
        self.dialog = Some(crate::Dialog::RedactBeforeSave);
    }

    /// Go on with save `p` once its marks are answered for: to the place the Save As picker gave,
    /// or as Save does. `check_marks` as for [`Self::save_then_checked`].
    fn resume_save(&mut self, index: usize, p: PendingRedactSave, check_marks: bool) {
        let PendingRedactSave { id, target, dest, after } = p;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dest) = dest {
            self.save_doc_to_then(id, &dest, target, after);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (id, dest);
        self.save_then_checked(index, target, check_marks, after);
    }

    /// [`Dialog::RedactBeforeSave`](crate::Dialog::RedactBeforeSave)'s Redact all and save: apply
    /// the marks, as Redact all does, then go on with the save. A refused apply saves nothing.
    pub(crate) fn redact_then_save(&mut self) {
        let Some(p) = self.pending_redact_save.take() else { return };
        let Some(index) = self.views.iter().position(|v| v.id == p.id) else { return };
        // The document being saved is the one in front (Save, or the close prompt brought it
        // forward); applying edits goes to the document in front.
        self.active = Some(index);
        if self.apply_edit(Edit::ApplyRedactions { pages: None }) {
            self.resume_save(index, p, true);
        }
    }

    /// [`Dialog::RedactBeforeSave`](crate::Dialog::RedactBeforeSave)'s Save with marks: write the
    /// marks as they are, for applying later, and don't ask again until the document changes.
    /// Only a save that writes counts: one cancelled on the way (the Save As picker, or the
    /// question about a file changed on disk) leaves the next Save asking again, as does that
    /// question's Save as…, which starts the save over.
    pub(crate) fn save_keeping_redaction_marks(&mut self) {
        let Some(p) = self.pending_redact_save.take() else { return };
        let Some(index) = self.views.iter().position(|v| v.id == p.id) else { return };
        let PendingRedactSave { id, target, dest, after } = p;
        // The marks are noted as kept once the file is written, at the generation it is saved
        // at (writing moves the document's generation on).
        let after = Box::new(move |app: &mut Self| {
            if let Some(generation) = app.session.get(id).map(|d| d.edit_generation()) {
                app.kept_redaction_marks.insert(id, generation);
            }
            after(app);
        });
        self.resume_save(index, PendingRedactSave { id, target, dest, after }, false);
    }

    /// Write document `id` to `dest` and make it the document's file. Returns `true` if written.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget) -> bool {
        // Save As answers on a later frame: commit what was typed meanwhile, too (#166), even
        // if another tab is active by then.
        if !self.commit_typing_in(id) || !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let Some((name, full)) = self.session.get(id).map(|d| {
            // Save As to another file is a clean copy, without the earlier revisions that still
            // hold deleted pages or replaced text (#2165). A signed document stays incremental so
            // its signatures keep covering the file.
            let full = target == SaveTarget::As
                && !d.path.as_deref().is_some_and(|p| crate::same_file(p, dest))
                && d.read_only_reason.is_none()
                && !d.is_signed();
            (d.name.clone(), full)
        }) else {
            self.notify_tr("The document was closed before it could be saved.");
            return false;
        };
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) };
        let bytes = match bytes.and_then(|b| self.session.check_save(id, &b).map(|()| b)) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return false;
            }
        };
        if let Err(e) = write_atomically(dest, &bytes) {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", dest), ("e", &e.to_string())]);
            return false;
        }
        // Our own write is not a change on disk to reload (#431).
        self.watch.known_now(id, dest);
        match self.session.mark_saved(id, bytes, Some(dest.to_string())) {
            Ok(()) => {
                self.forget_recovery(id);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
                {
                    view.document_changed(&doc.info);
                    // Saving over a version that changed on disk keeps this one: nothing to ask.
                    view.disk_changed = false;
                }
                self.notify_fmt("Saved {name}", &[("name", &short_name(dest))]);
                #[cfg(target_os = "macos")]
                crate::macos_association::repair(std::path::Path::new(dest));
                true
            }
            Err(e) => {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                false
            }
        }
    }
}

/// Whether `path` is the temporary copy an email client opened an attachment from, so that Save
/// asks where to keep the document instead of writing to it.
///
/// Outlook opens an attachment from its own cache: `INetCache\Content.Outlook\<random>\` for
/// classic Outlook (`Temporary Internet Files\Content.Outlook\` on older Windows), and the
/// `Packages\Microsoft.OutlookForWindows_<id>` package folder for the new Outlook. The patterns
/// follow those layouts, so an ordinary folder that merely shares a name doesn't match. Saving there either fails,
/// because Outlook can still hold the file, or appears to work while the edits stay out of the
/// email and are deleted with the cache. Apple Mail opens one from `Mail Downloads/<UUID>/` in
/// its sandbox container, where other apps can't write, so saving there fails (#2751). Matched on
/// either separator and in any case, so it can be tested on every platform.
pub(crate) fn is_mail_temp_copy(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    // Outlook's cache holds `<marker><random folder>/<file>`: exactly one folder, then the file.
    let in_outlook_cache = |marker: &str| {
        p.split_once(marker)
            .is_some_and(|(_, rest)| matches!(rest.split('/').collect::<Vec<_>>()[..], [dir, file] if !dir.is_empty() && !file.is_empty()))
    };
    in_outlook_cache("/inetcache/content.outlook/")
        || in_outlook_cache("/temporary internet files/content.outlook/")
        || p.contains("/packages/microsoft.outlookforwindows_")
        || p.contains("/library/containers/com.apple.mail/data/library/mail downloads/")
}

/// The folder a Save As style dialog should open in for a document stored at `path`: the
/// document's own folder, as Acrobat and Foxit do, instead of whichever folder the system picks.
/// `None` (keep the system's choice) for a new document, a path with no parent, or an email
/// attachment's temporary copy (`is_mail_temp_copy`), which is no place to keep work. Also right
/// for UNC paths on Windows (`\\server\share\dir\f.pdf` gives `\\server\share\dir`).
pub(crate) fn save_dialog_folder(path: Option<&str>) -> Option<std::path::PathBuf> {
    let path = path.filter(|p| !is_mail_temp_copy(p))?;
    let parent = std::path::Path::new(path).parent()?;
    (!parent.as_os_str().is_empty()).then(|| parent.to_path_buf())
}

/// `dialog` opened in the folder of the document stored at `path` ([`save_dialog_folder`]).
pub(crate) fn in_document_folder(dialog: rfd::AsyncFileDialog, path: Option<&str>) -> rfd::AsyncFileDialog {
    match save_dialog_folder(path) {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_atomically_with(path, bytes, staging_suffixes())
}

/// [`write_atomically`], trying the staging names that `suffixes` give.
pub(super) fn write_atomically_with(path: &str, bytes: &[u8], suffixes: impl IntoIterator<Item = u64>) -> std::io::Result<()> {
    write_atomically_syncing(path, bytes, suffixes, std::fs::File::sync_all)
}

/// [`write_atomically_with`], flushing the staging file with `sync_all` (the standard library's
/// in the app, a failing one in tests). The writer itself is the platform crate's, shared with
/// the tools and the CLI (#2166); the desktop app keeps its staging-name form.
pub(super) fn write_atomically_syncing(
    path: &str,
    bytes: &[u8],
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    pdfcraft_platform::staging::write_atomically_with(std::path::Path::new(path), bytes, StagingName::TagThenSuffix, suffixes, sync_all)
}

#[cfg(not(target_arch = "wasm32"))]
fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Offer bytes as a browser download.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}
