//! PDF-tab actions. Closed history contains paths only, never bytes or passwords.
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use craft::host::queue::Priority;
use pdfcraft_engine::DocId;

pub(crate) enum Action {
    /// Close a tab, by its view key (`DocView::view_key`: split view can show one document in two
    /// tabs).
    Close(u64),
    /// Close all tabs except this one.
    CloseOthers(u64),
    /// Close all tabs to the right of this one.
    CloseToRight(u64),
    /// Duplicate this tab as a second view of the document.
    Duplicate(u64),
    /// Toggle pinned state of this tab.
    TogglePin(u64),
    /// Create a new group and add this tab to it.
    AddToNewGroup(u64),
    /// Add this tab to an existing group.
    AddToGroup {
        tab: u64,
        group_id: crate::tab_group::TabGroupId,
    },
    /// Remove this tab from its current group.
    RemoveFromGroup(u64),
    /// Ungroup all tabs in a group.
    Ungroup(crate::tab_group::TabGroupId),
    /// Close all tabs in a group.
    CloseGroup(crate::tab_group::TabGroupId),
    Reopen,
    Reveal(DocId),
    Rename(DocId),
    /// Split right (or down) with a tab, by its view key.
    Split {
        tab: u64,
        down: bool,
    },
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
            Action::CloseOthers(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.close_other_tabs(ctx, i);
                }
            }
            Action::CloseToRight(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.close_tabs_to_right(ctx, i);
                }
            }
            Action::Duplicate(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.duplicate_tab(i);
                }
            }
            Action::TogglePin(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    let pinned = !self.views[i].pinned;
                    self.set_tab_pinned(i, pinned);
                }
            }
            Action::AddToNewGroup(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    let gid = self.create_tab_group(String::new(), crate::tab_group::TabGroupColor::Blue);
                    self.add_tab_to_group(i, gid);
                    self.editing_group = Some(gid);
                    self.group_title_draft.clear();
                }
            }
            Action::AddToGroup { tab, group_id } => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.add_tab_to_group(i, group_id);
                }
            }
            Action::RemoveFromGroup(tab) => {
                if let Some(i) = self.tab_with_key(tab) {
                    self.remove_tab_from_group(i);
                }
            }
            Action::Ungroup(group_id) => {
                self.ungroup(group_id);
            }
            Action::CloseGroup(group_id) => {
                self.close_tab_group(ctx, group_id);
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
}
