//! Explorer's Player-style verbs arrive one file per process. Review lists collect later
//! requests explicitly, without a timing heuristic that can lose a late selection.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellAction {
    Convert,
    Email,
    Combine,
}

impl ShellAction {
    pub fn code(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Email => "email",
            Self::Combine => "combine",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "convert" => Some(Self::Convert),
            "email" => Some(Self::Email),
            "combine" => Some(Self::Combine),
            _ => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub type OpenDraftFn = Box<dyn Fn(&std::path::Path) -> Result<(), String>>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct EmailRun {
    receiver: std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>,
    selected: Vec<String>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for EmailRun {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl crate::PdfCraftApp {
    pub fn receive_shell(&mut self, action: ShellAction, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        if paths.len() > pdfcraft_engine::email::MAX_FILES {
            self.notify_fmt("Explorer selection is limited to {count} files.", &[("count", "100")]);
            return;
        }
        // Dialog actions still target the active document. Never let an incoming request
        // switch it underneath Rotate/Redact/Export, a password or a close/save prompt.
        // Later requests join the queue too, preserving action boundaries and arrival order.
        if !self.shell_requests.is_empty() || self.shell_request_blocked(action) {
            let pending = self.shell_requests.iter().map(|(_, paths)| paths.len()).sum::<usize>();
            if paths.len() > pdfcraft_engine::email::MAX_FILES.saturating_sub(pending) {
                self.notify_fmt("Explorer selection is limited to {count} files.", &[("count", "100")]);
                return;
            }
            let bytes = self.shell_requests.iter().flat_map(|(_, paths)| paths).chain(paths).map(String::len).fold(0usize, usize::saturating_add);
            if bytes > 1024 * 1024 {
                self.notify(shell_memory_limit(1024 * 1024));
                return;
            }
            self.shell_requests.push_back((action, paths.to_vec()));
            return;
        }
        self.apply_shell_request(action, paths);
    }

    fn shell_request_blocked(&self, action: ShellAction) -> bool {
        self.dialog_open()
            || !self.pending_opens.is_empty()
            || self.palette_open
            || self.image_import.is_some()
            || (action != ShellAction::Email && (!self.shell_email_paths.is_empty() || self.shell_email_run.is_some()))
    }

    pub(crate) fn poll_shell_requests(&mut self) {
        while let Some((action, _)) = self.shell_requests.front() {
            if self.shell_request_blocked(*action) {
                return;
            }
            let Some((action, paths)) = self.shell_requests.pop_front() else { return };
            self.apply_shell_request(action, &paths);
        }
    }

    fn apply_shell_request(&mut self, action: ShellAction, paths: &[String]) {
        self.full_screen = false;
        match action {
            ShellAction::Convert | ShellAction::Combine => {
                for path in paths {
                    if action == ShellAction::Combine && self.combine_draft.len() >= pdfcraft_engine::email::MAX_FILES {
                        self.notify_fmt("Explorer selection is limited to {count} files.", &[("count", "100")]);
                        break;
                    }
                    if action == ShellAction::Combine {
                        self.stage_shell_combine(path, pdfcraft_engine::email::MAX_PDF_BYTES);
                    } else {
                        let result = pdfcraft_engine::guard(|| {
                            let limit = pdfcraft_engine::email::MAX_PDF_BYTES;
                            let used = self.shell_source_bytes();
                            if used >= limit {
                                return Err(shell_memory_limit(limit));
                            }
                            let (name, bytes) = pdfcraft_engine::email::read_source(std::path::Path::new(path))?;
                            if bytes.len() > limit.saturating_sub(used) {
                                return Err(shell_memory_limit(limit));
                            }
                            let pdf = self.session.as_pdf(&name, &std::sync::Arc::new(bytes)).map_err(|e| e.to_string())?;
                            if pdf.len() > limit.saturating_sub(used) {
                                return Err(shell_memory_limit(limit));
                            }
                            self.open_created_bytes(&format!("{}.pdf", name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem)), Ok(pdf))
                        })
                        .and_then(|result| result);
                        if let Err(e) = result {
                            self.notify_fmt("Couldn't open {name}: {e}", &[("name", path), ("e", &e)]);
                        }
                    }
                }
            }
            ShellAction::Email => {
                for path in paths {
                    // Dedupe only within this still-pending list. Saved or removed files can be
                    // deliberately added to a later draft; no timestamp-based suppression.
                    if self.shell_email_paths.contains(path) {
                        continue;
                    }
                    if self.shell_email_paths.len() >= pdfcraft_engine::email::MAX_FILES {
                        self.notify_fmt("Explorer selection is limited to {count} files.", &[("count", "100")]);
                        break;
                    }
                    self.shell_email_paths.push(path.clone());
                }
            }
        }
    }

    fn shell_source_bytes(&self) -> usize {
        self.combine_draft
            .iter()
            .map(|row| row.bytes.len())
            .chain(self.session.docs().iter().map(|doc| doc.bytes.len()))
            .fold(0usize, usize::saturating_add)
    }

    fn stage_shell_combine(&mut self, path: &str, limit: usize) {
        let used = self.shell_source_bytes();
        let memory_error = || shell_memory_limit(limit);
        let result = pdfcraft_engine::guard(|| {
            if used >= limit {
                return Err(memory_error());
            }
            let (name, bytes) = pdfcraft_engine::email::read_source(std::path::Path::new(path))?;
            if bytes.len() > limit.saturating_sub(used) {
                return Err(memory_error());
            }
            let pdf = self.session.as_pdf(&name, &std::sync::Arc::new(bytes)).map_err(|e| e.to_string())?;
            if pdf.len() > limit.saturating_sub(used) {
                return Err(memory_error());
            }
            Ok((name, pdf))
        })
        .and_then(|result| result);
        match result {
            Ok((name, bytes)) => {
                self.stage_combine_with(vec![crate::combine_ui::Incoming { name, bytes, modified: None, note: None, open_doc: None }])
            }
            Err(e) => {
                let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.into());
                // Failed sources stay visible, so Combine asks before leaving them out.
                self.stage_combine_failure(name, e);
            }
        }
    }

    fn start_shell_email(&mut self, selected: Vec<String>, parent: std::path::PathBuf, ctx: egui::Context) -> Result<(), String> {
        if self.shell_email_run.is_some() {
            return Err("an email draft is already being prepared".into());
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancel.clone();
        let paths = selected.clone();
        std::thread::Builder::new()
            .name("pdfcraft-email".into())
            .spawn(move || {
                let result = pdfcraft_engine::guard(|| {
                    let should_stop = || stop.load(std::sync::atomic::Ordering::Relaxed);
                    let sources = paths.iter().map(|path| {
                        if should_stop() { Err("cancelled".into()) } else { pdfcraft_engine::email::read_source(std::path::Path::new(path)) }
                    });
                    let draft = pdfcraft_engine::email::prepare_cancelable(&pdfcraft_engine::Session::default(), sources, &should_stop)?;
                    draft.save_in_cancelable(&parent, &should_stop)
                })
                .and_then(|result| result);
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.shell_email_run = Some(EmailRun { receiver, selected, cancel });
        Ok(())
    }

    fn poll_shell_email(&mut self) {
        let Some(run) = &self.shell_email_run else { return };
        let result = match run.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("email preparation stopped before returning a result".into()),
        };
        let Some(run) = self.shell_email_run.take() else { return };
        let cancelled = run.cancel.load(std::sync::atomic::Ordering::Relaxed);
        match result {
            Ok(path) => {
                self.shell_email_paths.retain(|path| !run.selected.contains(path));
                self.notify_fmt("Email draft saved to {path}", &[("path", &path.display().to_string())]);
                if !cancelled
                    && let Some(open) = &self.open_email_draft
                    && let Err(e) = open(&path)
                {
                    self.notify_fmt("Couldn't open the email draft: {e}", &[("e", &e)]);
                }
            }
            Err(_) if cancelled => self.notify(tl!("Cancelled")),
            Err(e) => self.notify_fmt("Couldn't prepare the email draft: {e}", &[("e", &e)]),
        }
    }

    pub(crate) fn shell_email_window(&mut self, ctx: &egui::Context) {
        self.poll_shell_email();
        // An IPC request must not cover a password, recovery or already-open default-app
        // question. Poll the worker above even while its review is hidden.
        if self.dialog_open() || self.palette_open || self.shell_email_paths.is_empty() {
            return;
        }
        let busy = self.shell_email_run.is_some();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut remove = None;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(tl!("Prepare email draft")).id(egui::Id::unique("shell-email")).default_width(510.0).resizable(true).collapsible(false).show(ctx, |ui| {
            crate::widgets::begin_dialog(ui);
            ui.label(tl!("PDF attachments"));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for (i, path) in self.shell_email_paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new(tl!("Remove")).small()).clicked() { remove = Some(i); }
                        let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy()).unwrap_or_else(|| path.as_str().into());
                        ui.add(egui::Label::new(name).truncate()).on_hover_text(path);
                    });
                }
            });
            ui.label(tl!("The draft contains PDF attachments. Open it with a mail app that supports .eml drafts; in Outlook.com, create a message and attach the saved PDFs yourself."));
            ui.horizontal_wrapped(|ui| {
                save = ui.add_enabled_ui(!busy, |ui| crate::widgets::pill_button(ui, tl!("Save email draft…"), true)).inner.clicked();
                cancel = crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked();
                if busy { ui.spinner(); }
                if self.shell_email_run.as_ref().is_some_and(|run| run.cancel.load(std::sync::atomic::Ordering::Relaxed)) {
                    ui.label(tl!("Cancelling…"));
                }
            });
        });
        if cancel {
            if let Some(run) = &self.shell_email_run {
                run.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            } else {
                self.shell_email_paths.clear();
                self.shell_email_generation = self.shell_email_generation.wrapping_add(1);
            }
        } else if let Some(i) = remove {
            self.shell_email_paths.remove(i);
            self.shell_email_generation = self.shell_email_generation.wrapping_add(1);
        }
        if save && !self.shell_email_paths.is_empty() {
            let paths = self.shell_email_paths.clone();
            let generation = self.shell_email_generation;
            let ctx = ctx.clone();
            let save = move |app: &mut Self, parent: std::path::PathBuf| {
                if app.shell_email_generation != generation {
                    return;
                }
                if let Err(e) = app.start_shell_email(paths, parent, ctx) {
                    app.notify_fmt("Couldn't prepare the email draft: {e}", &[("e", &e)]);
                }
            };
            if let Some(parent) = self.export_dir_override.clone() {
                save(self, std::path::PathBuf::from(parent));
            } else {
                self.ask_one(crate::pickers::Ask::Folder(rfd::AsyncFileDialog::new().set_title(tl!("Save email draft…"))), None, save);
            }
        }
    }

    /// Blocking counterpart for headless callers. The interactive window uses a background worker.
    /// Failure retains the pending list; late arrivals survive a successful confirmed selection.
    pub fn save_shell_email(&mut self, selected: &[String], parent: &std::path::Path) -> Result<std::path::PathBuf, String> {
        let path = pdfcraft_engine::guard(|| {
            let sources = selected.iter().map(|path| pdfcraft_engine::email::read_source(std::path::Path::new(path)));
            pdfcraft_engine::email::prepare(&self.session, sources)?.save_in(parent)
        })
        .and_then(|result| result)?;
        self.shell_email_paths.retain(|path| !selected.contains(path));
        Ok(path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn shell_memory_limit(limit: usize) -> String {
    crate::i18n::fmt(tl!("Explorer selection exceeds the {size} MiB memory limit."), &[("size", &(limit / (1024 * 1024)).to_string())])
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    #[test]
    fn permissions_password_prompt_defers_every_explorer_action_until_cancelled() {
        let directory = std::env::temp_dir().join(format!("pdfcraft-shell-permissions-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("notes.txt");
        std::fs::write(&source, b"Synthetic Explorer conversion").unwrap();
        let path = source.to_string_lossy().into_owned();
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let pdf = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("original.pdf", None, pdf.as_ref().clone()).unwrap();
        app.request_unlock(None);
        for action in [super::ShellAction::Convert, super::ShellAction::Combine, super::ShellAction::Email] {
            app.receive_shell(action, std::slice::from_ref(&path));
        }
        app.poll_shell_requests();
        assert!(app.unlock_prompt.is_some());
        assert_eq!(app.active, Some(0));
        assert_eq!(app.views.len(), 1);
        assert!(app.combine_draft.is_empty() && app.shell_email_paths.is_empty());
        assert_eq!(app.shell_requests.len(), 3);
        app.submit_unlock(None);
        app.poll_shell_requests();
        assert_eq!(app.views.len(), 2);
        assert_eq!(app.active, None, "the later Combine action remains selected");
        assert_eq!(app.combine_draft.len(), 1);
        assert_eq!(app.shell_email_paths, [path]);
        assert!(app.shell_requests.is_empty());
    }

    #[test]
    fn worker_cancel_retains_review_and_late_arrivals_without_opening_mail() {
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.receive_shell(super::ShellAction::Email, &["first.txt".into()]);
        let (tx, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        app.shell_email_run = Some(super::EmailRun { receiver, selected: app.shell_email_paths.clone(), cancel });
        app.open_email_draft = Some(Box::new(|_| panic!("cancelled worker must not open a client")));
        app.receive_shell(super::ShellAction::Email, &["later.txt".into()]);
        assert!(app.modal_open());
        app.poll_shell_email(); // An unfinished worker is polled without blocking the window.
        assert!(app.shell_email_run.is_some());
        tx.send(Err("cancelled".into())).unwrap();
        app.poll_shell_email();
        assert!(app.shell_email_run.is_none());
        assert_eq!(app.shell_email_paths, ["first.txt", "later.txt"]);
        assert!(app.toast.as_ref().unwrap().0.contains("Cancelled"));
    }

    #[test]
    fn combine_budget_counts_existing_rows_and_late_arrivals_and_keeps_failures_visible() {
        let directory = std::env::temp_dir().join(format!("pdfcraft-combine-budget-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("notes.txt");
        std::fs::write(&source, b"one").unwrap();
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let path = source.to_string_lossy();
        app.stage_shell_combine(&path, 1024 * 1024);
        let used = app.combine_draft[0].bytes.len();
        assert!(used > 0);
        app.stage_shell_combine(&path, used);
        app.stage_shell_combine(&path, used + 1);
        app.stage_shell_combine(&path, used + 4); // Raw text fits; its converted PDF does not.
        assert_eq!(app.combine_draft.len(), 4);
        assert!(
            app.combine_draft[1..].iter().all(|row| row.bytes.is_empty() && row.problem.as_ref().is_some_and(|note| note.contains("memory limit")))
        );
        assert_eq!(app.combine_draft.iter().map(|row| row.bytes.len()).sum::<usize>(), used);
        app.stage_shell_combine(&directory.join("missing.txt").to_string_lossy(), 1024 * 1024);
        let missing = app.combine_draft.last().unwrap();
        assert_eq!(missing.name, "missing.txt");
        assert!(missing.bytes.is_empty() && missing.problem.is_some() && missing.pages == 0);
    }

    #[test]
    fn deferred_requests_are_bounded_and_empty_requests_do_not_fill_the_queue() {
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.dialog = Some(crate::Dialog::About);
        app.receive_shell(super::ShellAction::Convert, &[]);
        assert!(app.shell_requests.is_empty());
        let paths: Vec<_> = (0..100).map(|i| format!("source-{i}.txt")).collect();
        app.receive_shell(super::ShellAction::Convert, &paths);
        app.receive_shell(super::ShellAction::Combine, &["overflow.txt".into()]);
        assert_eq!(app.shell_requests.len(), 1);
        assert_eq!(app.shell_requests[0].1, paths);
        assert!(app.toast.as_ref().unwrap().0.contains("100"));
        app.shell_requests.clear();
        app.receive_shell(super::ShellAction::Convert, &["x".repeat(1024 * 1024 + 1)]);
        assert!(app.shell_requests.is_empty());
        assert!(app.toast.as_ref().unwrap().0.contains("memory limit"));
        assert!(app.session.docs().is_empty());
    }
}
