//! PDF-tab actions. Closed history contains paths only, never bytes or passwords.
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use craft::host::queue::Priority;
use pdfcraft_engine::DocId;

pub(crate) enum Action {
    /// Close a tab, by its view key (`DocView::view_key`: split view can show one document in two
    /// tabs).
    Close(u64),
    Reopen,
    Reveal(DocId),
    Rename(DocId),
    /// Split right (or down) with a tab, by its view key.
    Split {
        tab: u64,
        down: bool,
    },
    /// Move a tab to a window of its own, by its view key (#1683).
    NewWindow(u64),
}

/// Starts a PdfCraft window of its own with these arguments: a test's stand-in, or else a new run of this program.
pub type WindowLauncher = std::sync::Arc<dyn Fn(&[String]) -> Result<(), String> + Send + Sync>;

/// A new run of this program with `args`, which opens a window of its own (`--new-window` keeps it from handing the
/// file back to this one). The child isn't waited for: it outlives this window.
#[cfg(not(target_arch = "wasm32"))]
fn launch_window(args: &[String]) -> Result<(), String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(program).args(args).spawn().map(drop).map_err(|e| e.to_string())
}

/// Reveal a local file without opening it or changing document/Recent state. The file manager call
/// blocks (a disconnected network mount, a slow `dbus-send` or `explorer.exe`), so it runs as a
/// job on the host's queue; a failure is shown by [`PdfCraftApp::drain_reveals`] on the pass that
/// receives the result.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reveal_on_jobs(app: &mut crate::PdfCraftApp, path: PathBuf) {
    let target = path.clone();
    app.reveal_jobs.submit(path, Priority::VISIBLE, move |_| craft::platform::open::reveal(&target));
}

/// The browser has no file manager to show a file in: the web says so instead.
#[cfg(target_arch = "wasm32")]
pub(crate) fn reveal_on_jobs(app: &mut crate::PdfCraftApp, _path: PathBuf) {
    app.notify_error("The browser can't show files in a folder.".to_string());
}

impl crate::PdfCraftApp {
    /// Shows the failures of finished reveal jobs; once a frame, whatever the window shows.
    pub(crate) fn drain_reveals(&mut self) {
        while let Some(done) = self.reveal_jobs.try_recv() {
            if let Err(e) = done.result {
                self.notify_error(e);
            }
        }
    }

    /// Drops the latest closed-tab entry of `path`, once Reopen Closed Tab has brought it back.
    pub(crate) fn forget_closed_tab(&mut self, path: &str) {
        if let Some(at) = self.closed_tabs.iter().rposition(|closed| closed == path) {
            self.closed_tabs.remove(at);
        }
    }

    pub(crate) fn tab_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Close(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.request_close_tab(ctx, i);
                }
            }
            Action::Split { tab, down } => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.split_tab(ctx, i, down);
                }
            }
            Action::Reopen => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = self.closed_tabs.last().cloned() {
                    // A tab that shows the file already (opened again through File > Open) comes to the front.
                    // Otherwise the open is queued, and its entry goes only once the file has opened.
                    match self.open_path(&path) {
                        crate::OpenOutcome::Opened | crate::OpenOutcome::Focused => self.forget_closed_tab(&path),
                        crate::OpenOutcome::Pending => {
                            if let Some(pending) = self.pending_opens.iter_mut().find(|p| p.path.as_deref() == Some(path.as_str())) {
                                pending.reopen = true;
                            }
                        }
                        crate::OpenOutcome::Failed(_) => {}
                    }
                }
            }
            Action::Reveal(id) => {
                if let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) {
                    reveal_on_jobs(self, PathBuf::from(path));
                }
            }
            Action::Rename(id) => self.start_rename(ctx, id),
            Action::NewWindow(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.move_tab_to_new_window(i);
                }
            }
        }
    }

    /// Moves tab `index` to a window of its own (#1683), as dragging it out of the window does: the document opens
    /// there at the page this tab shows (and its zoom, unless the view fits the page or its width, which the new
    /// window fits to its own size), and the tab closes here once the window is on its way. The new window opens the
    /// file, so a document with changes that aren't saved, or never saved to a file, stays here with a notice: the
    /// changes would not come along. A document under OCR review stays too, as for Close.
    pub(crate) fn move_tab_to_new_window(&mut self, index: usize) {
        let Some(view) = self.views.get(index) else { return };
        let (page, zoom, fitted) = (view.current().saturating_add(1), (view.zoom() * 100.0).round(), view.fit() != crate::canvas::Fit::None);
        let Some(path) = self.session.get(view.id).and_then(|d| d.path.clone()) else {
            self.notify_tr("Save this document to a file to move it to a new window");
            return;
        };
        if self.has_unsaved_work(index) {
            self.notify_tr("Save your changes to move this document to a new window");
            return;
        }
        if self.hold_close_for_ocr_review(index) {
            return;
        }
        let mut args = vec!["--new-window".to_owned(), "--page".to_owned(), page.to_string()];
        if !fitted && zoom.is_finite() && zoom > 0.0 {
            args.extend(["--zoom".to_owned(), format!("{zoom}")]);
        }
        args.push(path);
        let launched = match &self.window_launcher {
            Some(launch) => launch(&args),
            #[cfg(not(target_arch = "wasm32"))]
            None => launch_window(&args),
            // A browser tab has no second window to start.
            #[cfg(target_arch = "wasm32")]
            None => Err("The browser can't open another window.".to_owned()),
        };
        match launched {
            Ok(()) => self.close_tab(index),
            Err(e) => self.notify_fmt("Couldn't open a new window: {e}", &[("e", &e)]),
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::PdfCraftApp;
    use egui::accesskit::Role;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    use std::sync::Arc;

    const PDF: &[u8] = include_bytes!("../../tests/data/form.pdf");

    /// An English app on a host whose jobs run when the test says.
    fn app_on(jobs: Arc<craft::host::ManualJobs>) -> PdfCraftApp {
        let info = craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" };
        let host = craft::host::Host::builder(info).jobs(jobs).build();
        let wake = craft::host::Wake::none();
        let (intake, _) = craft::platform::open::FileIntake::new(wake.clone());
        let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
        let watch = craft::platform::watch::FileWatch::new(wake.clone());
        let services = crate::AppServices::new(wake, clipboard, None, None, intake, watch);
        let mut app = PdfCraftApp::with_host(Arc::new(host), services);
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app
    }

    /// Runs the queued opens and takes their results, as the next frame would.
    fn settle(app: &mut PdfCraftApp, jobs: &craft::host::ManualJobs) {
        jobs.run_all();
        app.poll_pending_opens(&egui::Context::default());
    }

    /// An English app whose jobs never run.
    fn app() -> PdfCraftApp {
        app_on(Arc::new(craft::host::ManualJobs::new()))
    }

    /// The unsaved-changes dialog's "don't keep the changes" and Cancel answers as the framework words
    /// them on this OS.
    fn answers() -> (String, String) {
        // PLAN: assumed paths `craft::ui::dialog::DialogWords`, `craft::ui::runtime::words::CommonWords`,
        // `craft::i18n::WordSet`; their fields display as their English text.
        use craft::i18n::WordSet as _;
        let dialog = craft::ui::dialog::DialogWords::english();
        let common = craft::ui::runtime::words::CommonWords::english();
        // The app words the question as production does on every platform: "Don't save", in lower case.
        let _ = dialog;
        ("Don't save".to_string(), common.cancel.as_str().to_string())
    }

    #[test]
    fn confirmed_close_uses_id_and_cancel_keeps_history_empty() {
        let ctx = &egui::Context::default();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = app();
                app.open_bytes(ctx, "first.pdf", Some("/tmp/first.pdf".into()), PDF.to_vec()).unwrap();
                app.open_bytes(ctx, "second.pdf", None, PDF.to_vec()).unwrap();
                app
            }
        });
        h.run_steps(4);
        let (first, first_tab) = (h.state().views[0].id, h.state().views[0].view_key());
        h.state_mut().session.apply(first, pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }).unwrap();
        h.state_mut().tab_action(&egui::Context::default(), Action::Close(first_tab));
        h.run_steps(3);
        let (dont_save, cancel) = answers();
        assert_eq!(h.state().views.len(), 2);
        assert!(h.state().closed_tabs.is_empty());
        h.get_by_role_and_label(Role::Button, &cancel).click();
        h.run_steps(3);
        assert_eq!(h.state().views.len(), 2);
        assert!(h.state().closed_tabs.is_empty());
        h.state_mut().tab_action(&egui::Context::default(), Action::Close(first_tab));
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, &dont_save).click();
        h.run_steps(3);
        assert_eq!(h.state().views.len(), 1);
        assert_eq!(h.state().closed_tabs, ["/tmp/first.pdf"]);
        assert_eq!(h.state().active, Some(0));
        h.state_mut().close_tab(0);
        assert_eq!(h.state().closed_tabs.len(), 1, "pathless tabs are not recorded");
    }

    #[test]
    fn reopen_reads_saved_file_and_history_is_bounded() {
        use std::io::Write as _;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("pdfcraft-tab-{}-{nonce}-年度 report.pdf", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        file.write_all(PDF).unwrap();
        drop(file);
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let mut app = app_on(jobs.clone());
        let path_text = path.to_str().unwrap();
        for _ in 0..12 {
            app.open_path(path_text);
            settle(&mut app, &jobs);
            app.close_tab(0);
        }
        assert_eq!(app.closed_tabs.len(), 10);
        app.tab_action(&egui::Context::default(), Action::Reopen);
        assert_eq!(app.closed_tabs.len(), 10, "the entry stays until the file has opened");
        settle(&mut app, &jobs);
        assert_eq!(app.views.len(), 1);
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.path.as_deref(), Some(path_text));
        assert!(!doc.dirty);
        assert_eq!(app.closed_tabs.len(), 9);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_reopen_keeps_history_and_retry_does_not_duplicate_tabs() {
        use std::io::Write as _;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("pdfcraft-retry-{}-{nonce}.pdf", std::process::id()));
        let text = path.to_str().unwrap();
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
        file.write_all(PDF).unwrap();
        drop(file);
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let mut app = app_on(jobs.clone());
        app.open_path(text);
        settle(&mut app, &jobs);
        app.close_tab(0);
        std::fs::remove_file(&path).unwrap();
        app.tab_action(&egui::Context::default(), Action::Reopen);
        settle(&mut app, &jobs);
        assert!(app.views.is_empty());
        assert_eq!(app.closed_tabs, [text]);
        std::fs::write(&path, PDF).unwrap();
        app.tab_action(&egui::Context::default(), Action::Reopen);
        settle(&mut app, &jobs);
        assert_eq!(app.views.len(), 1);
        assert!(app.closed_tabs.is_empty());
        app.closed_tabs.push(text.into());
        app.tab_action(&egui::Context::default(), Action::Reopen);
        assert_eq!(app.views.len(), 1, "an already-open file is selected, not duplicated");
        assert!(app.closed_tabs.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_failed_reveal_is_reported_back_to_the_ui_thread() {
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let held = jobs.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            app_on(held)
        });
        h.run_steps(2);
        reveal_on_jobs(h.state_mut(), PathBuf::from("/pdfcraft-no-such-dir/missing.pdf"));
        assert_eq!(jobs.waiting(), 1, "the reveal is a job on the host");
        h.run_steps(2);
        let posted = |h: &Harness<'_, PdfCraftApp>| crate::notices::posted(&h.ctx).len();
        assert_eq!(posted(&h), 0, "nothing is shown until the job ends");
        jobs.run_all();
        h.run_steps(2);
        assert_eq!(posted(&h), 1, "the failure is shown as a notice");
    }

    #[test]
    fn inactive_tab_menu_closes_target_and_disables_unavailable_actions() {
        let ctx = &egui::Context::default();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = app();
                for name in ["first.pdf", "second.pdf"] {
                    app.open_bytes(ctx, name, None, PDF.to_vec()).unwrap();
                }
                app
            }
        });
        h.run_steps(8);
        h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").click_secondary();
        h.run_steps(2);
        assert!(h.get_by_label("Reopen Closed Tab").accesskit_node().is_disabled());
        let reveal_label = if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Reveal in File Explorer" };
        assert!(h.get_by_label(reveal_label).accesskit_node().is_disabled());
        assert_eq!(h.state().active, Some(1));
        h.get_by_label("Close Tab").click();
        h.run_steps(3);
        assert_eq!(h.state().views.len(), 1);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().name, "second.pdf");
    }

    /// #1683: Move to New Window starts a window of its own on the tab's file, at its page (and zoom, when the view
    /// doesn't fit the page or width), and the tab closes here. A tab whose changes aren't saved, or that has no file,
    /// stays, and so does one whose window couldn't be started.
    #[test]
    fn move_to_new_window_starts_a_window_on_the_file_and_closes_the_tab() {
        let ctx = &egui::Context::default();
        let launched: Arc<std::sync::Mutex<Vec<Vec<String>>>> = Arc::default();
        let record = launched.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = app();
            for (name, path) in [
                ("first.pdf", Some("/tmp/first.pdf")),
                ("unsaved.pdf", None),
                ("edited.pdf", Some("/tmp/edited.pdf")),
                ("third.pdf", Some("/tmp/third.pdf")),
                ("fourth.pdf", Some("/tmp/fourth.pdf")),
            ] {
                // A document of three pages, so the page shown can be other than the first.
                let bytes = if name == "third.pdf" { app.session.create_blank(200.0, 300.0, 3).unwrap().to_vec() } else { PDF.to_vec() };
                app.open_bytes(ctx, name, path.map(String::from), bytes).unwrap();
            }
            let record = record.clone();
            app.window_launcher = Some(Arc::new(move |args: &[String]| {
                record.lock().unwrap().push(args.to_vec());
                Ok(())
            }));
            app
        });
        h.run_steps(8);
        let key = |h: &Harness<'_, PdfCraftApp>, name: &str| {
            let state = h.state();
            state.views.iter().find(|v| state.session.get(v.id).is_some_and(|d| d.name == name)).map(|v| v.view_key())
        };

        // From the tab's menu: the file, at the page shown; the view fits its width, so no zoom goes along.
        h.get_by_role_and_label(Role::Tab, "first.pdf").click_secondary();
        h.run_steps(2);
        h.get_by_label("Move to New Window").click();
        h.run_steps(3);
        assert_eq!(*launched.lock().unwrap(), [["--new-window", "--page", "1", "/tmp/first.pdf"]]);
        assert!(key(&h, "first.pdf").is_none(), "the tab moved out of this window");

        // A document never saved to a file has nothing for the new window to open.
        h.get_by_role_and_label(Role::Tab, "unsaved.pdf").click_secondary();
        h.run_steps(2);
        assert!(h.get_by_label("Move to New Window").accesskit_node().is_disabled());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        let unsaved = key(&h, "unsaved.pdf").unwrap();
        h.state_mut().tab_action(ctx, Action::NewWindow(unsaved));
        assert_eq!(launched.lock().unwrap().len(), 1);
        assert!(key(&h, "unsaved.pdf").is_some());

        // Changes that aren't saved would not come along: the tab stays.
        let edited = key(&h, "edited.pdf").unwrap();
        let id = h.state().views[h.state().tab_with_key(edited).unwrap()].id;
        h.state_mut().session.apply(id, pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }).unwrap();
        h.state_mut().tab_action(ctx, Action::NewWindow(edited));
        assert_eq!(launched.lock().unwrap().len(), 1, "no window for a document with unsaved changes");
        assert!(key(&h, "edited.pdf").is_some());

        // At a zoom of its own, on page 2: both go along.
        let third = key(&h, "third.pdf").unwrap();
        let index = h.state().tab_with_key(third).unwrap();
        {
            let view = &mut h.state_mut().views[index];
            view.go_to_page(1);
            view.view.zoom = 1.5;
            view.view.fit = crate::canvas::Fit::None;
        }
        h.state_mut().tab_action(ctx, Action::NewWindow(third));
        assert_eq!(launched.lock().unwrap().last().unwrap(), &["--new-window", "--page", "2", "--zoom", "150", "/tmp/third.pdf"]);
        assert!(key(&h, "third.pdf").is_none());

        // A window that can't be started leaves the tab where it is.
        h.state_mut().window_launcher = Some(Arc::new(|_: &[String]| Err("no".to_owned())));
        let fourth = key(&h, "fourth.pdf").unwrap();
        h.state_mut().tab_action(ctx, Action::NewWindow(fourth));
        assert!(key(&h, "fourth.pdf").is_some(), "the tab stays when its window can't start");
    }

    /// #1683, #2848: a tab dragged out of the strip and let go where no strip or pane takes it (here the tools panel)
    /// moves to a window of its own.
    #[test]
    fn a_tab_dragged_out_of_the_strip_moves_to_a_new_window() {
        let ctx = &egui::Context::default();
        let launched: Arc<std::sync::Mutex<Vec<Vec<String>>>> = Arc::default();
        let record = launched.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = app();
            for name in ["first.pdf", "second.pdf"] {
                app.open_bytes(ctx, name, Some(format!("/tmp/{name}")), PDF.to_vec()).unwrap();
            }
            let record = record.clone();
            app.window_launcher = Some(Arc::new(move |args: &[String]| {
                record.lock().unwrap().push(args.to_vec());
                Ok(())
            }));
            app
        });
        h.run_steps(8);
        let from = h.get_by_role_and_label(Role::Tab, "first.pdf").rect().center();
        // Down on the tab, out of the strip in steps, and up over the tools panel at the left.
        let to = egui::pos2(120.0, 600.0);
        let button =
            |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        h.event(egui::Event::PointerMoved(from));
        h.event(button(from, true));
        h.run_steps(1);
        for step in 1..=12 {
            h.event(egui::Event::PointerMoved(from.lerp(to, step as f32 / 12.0)));
            h.run_steps(1);
        }
        h.event(button(to, false));
        h.run_steps(4);
        assert_eq!(*launched.lock().unwrap(), [["--new-window", "--page", "1", "/tmp/first.pdf"]]);
        let names: Vec<String> = h.state().views.iter().filter_map(|v| h.state().session.get(v.id).map(|d| d.name.clone())).collect();
        assert_eq!(names, ["second.pdf"]);
    }
}
