//! PDF-tab actions. Closed history and pinned tabs contain paths only, never bytes or passwords.
use pdfcraft_engine::DocId;

pub(crate) enum Action {
    Close(DocId),
    Reopen,
    Reveal(DocId),
    /// Pin the tab (#2926): its file opens at every start, first in the tab strip.
    Pin(DocId),
    Unpin(DocId),
}

/// The most files kept pinned: a settings file listing more is read up to here.
const MAX_PINNED: usize = 64;

/// How a pinned file is kept: its absolute path, so a file opened by a relative path matches.
/// The web has no paths to make absolute.
fn pin_key(path: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::last_session::absolute(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        path.to_string()
    }
}

/// The pinned files a settings file lists: text entries only, each once, at most [`MAX_PINNED`].
pub(crate) fn pinned_from_json(v: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in v.as_array().into_iter().flatten().filter_map(serde_json::Value::as_str) {
        if out.len() >= MAX_PINNED {
            break;
        }
        if !path.is_empty() {
            // Normalize on restore just as when pinning an open document, so a
            // relative persisted path can still be matched and unpinned.
            let path = pin_key(path);
            if !out.iter().any(|p| p == &path) {
                out.push(path);
            }
        }
    }
    out
}

impl crate::PdfCraftApp {
    /// Whether document `id`'s file is pinned.
    pub fn tab_pinned(&self, id: DocId) -> bool {
        self.session.get(id).and_then(|d| d.path.as_deref()).is_some_and(|p| {
            let p = pin_key(p);
            self.pinned_tabs.contains(&p)
        })
    }

    /// Puts the pinned tabs first, in the order they are in, and keeps the active tab (#2926).
    /// A file opened while it is pinned is added at the end, as any other: the steps that follow
    /// an open (a link's destination, a recovery, the file watcher) act on the newest tab, so the
    /// tab joins the pinned ones afterwards, here, once a frame.
    pub fn pinned_tabs_first(&mut self) {
        if self.pinned_tabs.is_empty() {
            return;
        }
        let active = self.active_ids().map(|(_, id)| id);
        let mut views = std::mem::take(&mut self.views);
        // Stable sorting keeps unpinned tabs in their existing order, while pinned tabs follow
        // the user's saved pin order. This also handles a pinned file opened from the command line
        // after the other pinned files were restored.
        views.sort_by_key(|v| {
            self.session
                .get(v.id)
                .and_then(|d| d.path.as_deref())
                .map(pin_key)
                .and_then(|path| self.pinned_tabs.iter().position(|p| p == &path))
                .map_or((1usize, 0usize), |index| (0, index))
        });
        self.views = views;
        if let Some(id) = active {
            self.active = self.views.iter().position(|v| v.id == id);
        }
    }

    /// `ui.set tab-pinned true|false` (#2926): pins or unpins the active tab, as its context menu
    /// does, for the control channel. Refused, with the reason, for a document without a file
    /// (nothing to open at the next start) and past [`MAX_PINNED`].
    pub(crate) fn set_tab_pinned(&mut self, value: &str) -> Result<(), String> {
        let pin = match value {
            "true" => true,
            "false" => false,
            _ => return Err("tab-pinned must be true or false".into()),
        };
        let (_, id) = self.active_ids().ok_or("tab-pinned: no document is open")?;
        if self.session.get(id).and_then(|d| d.path.as_deref()).is_none() {
            return Err("tab-pinned: only a document saved to a file can be pinned".into());
        }
        if pin && !self.tab_pinned(id) && self.pinned_tabs.len() >= MAX_PINNED {
            return Err(format!("tab-pinned: at most {MAX_PINNED} tabs can be pinned; unpin one first"));
        }
        self.tab_action(if pin { Action::Pin(id) } else { Action::Unpin(id) });
        Ok(())
    }

    pub(crate) fn tab_action(&mut self, action: Action) {
        match action {
            Action::Close(id) => {
                if let Some(i) = self.views.iter().position(|v| v.id == id) {
                    self.request_close_tab(i);
                }
            }
            Action::Reopen => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = self.closed_tabs.last().cloned() {
                    // A tab may already have been reopened through File > Open.
                    if let Some(index) = self.views.iter().position(|v| self.session.get(v.id).is_some_and(|d| d.path.as_deref() == Some(&path))) {
                        self.active = Some(index);
                        self.closed_tabs.pop();
                    } else {
                        let tabs = self.views.len();
                        self.open_path(&path);
                        // open_path reports errors through notifications, not a Result.
                        // Keep the entry until an actual document tab has been opened.
                        if self.views.len() > tabs {
                            self.closed_tabs.pop();
                        }
                    }
                }
            }
            Action::Pin(id) => {
                let Some(path) = self.session.get(id).and_then(|d| d.path.as_deref()).map(pin_key) else { return };
                if !self.pinned_tabs.contains(&path) {
                    if self.pinned_tabs.len() >= MAX_PINNED {
                        self.notify_tr("No more tabs can be pinned: unpin one first");
                        return;
                    }
                    self.pinned_tabs.push(path);
                }
                // To the end of the pinned tabs, which lead the strip.
                if let Some(from) = self.views.iter().position(|v| v.id == id) {
                    let to = self.views.iter().take(from).filter(|v| self.tab_pinned(v.id)).count();
                    self.move_tab(from, to);
                }
            }
            Action::Unpin(id) => {
                if let Some(path) = self.session.get(id).and_then(|d| d.path.as_deref()).map(pin_key) {
                    self.pinned_tabs.retain(|p| *p != path);
                }
            }
            Action::Reveal(id) => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = self.session.get(id).and_then(|d| d.path.clone())
                    && let Err(e) = reveal(std::path::Path::new(&path))
                {
                    self.notify_error(e);
                }
                #[cfg(target_arch = "wasm32")]
                let _ = id;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn reveal(path: &std::path::Path) -> Result<(), String> {
    // Absolute paths also prevent a filename starting with '-' becoming an option.
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Err("The file no longer exists".into());
    }
    let platform = std::env::consts::OS;
    let run =
        |program: &str, args: &[std::ffi::OsString]| reveal_process(program, args).output().map(|o| o.status.success()).map_err(|e| e.to_string());
    if matches!(platform, "linux" | "freebsd") {
        // `dbus-send --print-reply` waits up to 2 s for the file manager (then xdg-open may run),
        // so it runs off the UI thread; a failure there is only logged.
        let spawned = std::thread::Builder::new().name("pdfcraft-reveal".into()).spawn(move || {
            if let Err(e) = reveal_via(platform, &path, run) {
                log::warn!("reveal {}: {e}", path.display());
            }
        });
        return spawned.map(|_| ()).map_err(|e| e.to_string());
    }
    reveal_via(platform, &path, run)
}

/// The process that runs `program` with `args`. Windows: `reveal_command` already quotes what
/// Explorer needs quoted, and Rust's own quoting of an argument with a space breaks `/select,`
/// (#2579), so the arguments go on the command line as written.
#[cfg(not(target_arch = "wasm32"))]
fn reveal_process(program: &str, args: &[std::ffi::OsString]) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        for arg in args {
            command.raw_arg(arg);
        }
    }
    #[cfg(not(windows))]
    command.args(args);
    command
}

#[cfg(not(target_arch = "wasm32"))]
fn reveal_via(
    platform: &str,
    path: &std::path::Path,
    mut run: impl FnMut(&str, &[std::ffi::OsString]) -> Result<bool, String>,
) -> Result<(), String> {
    let (program, args) = reveal_command(platform, path)?;
    if run(program, &args).unwrap_or(false) {
        return Ok(());
    }
    if matches!(platform, "linux" | "freebsd") {
        // FileManager1 is optional. If absent, open the containing folder with xdg-open.
        let parent = path.parent().ok_or("The file has no parent directory")?;
        if run("xdg-open", &[parent.as_os_str().into()])? {
            return Ok(());
        }
    }
    Err("Could not reveal the file in the system file manager".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn reveal_command(platform: &str, path: &std::path::Path) -> Result<(&'static str, Vec<std::ffi::OsString>), String> {
    match platform {
        "windows" => {
            // Explorer reads `/select,"<object>"`: the path quoted after the comma (#2579). Quoting
            // the whole argument instead ("/select,C:\a b.pdf", what an ordinary argument with a
            // space becomes) opens Documents. A Windows path can't contain `"`, so the quotes
            // can't be closed early; one that does is refused.
            if path.as_os_str().as_encoded_bytes().contains(&b'"') {
                return Err("The file name can't be shown in File Explorer".into());
            }
            let mut selection = std::ffi::OsString::from("/select,\"");
            selection.push(path);
            selection.push("\"");
            Ok(("explorer.exe", vec![selection]))
        }
        "macos" => Ok(("open", vec!["-R".into(), path.as_os_str().into()])),
        "linux" | "freebsd" => {
            let mut uri = String::from("array:string:file://");
            for b in path.as_os_str().as_encoded_bytes() {
                if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
                    uri.push(char::from(*b));
                } else {
                    use std::fmt::Write as _;
                    let _ = write!(uri, "%{b:02X}");
                }
            }
            Ok((
                "dbus-send",
                vec![
                    "--session".into(),
                    "--print-reply".into(),
                    "--reply-timeout=2000".into(),
                    "--dest=org.freedesktop.FileManager1".into(),
                    "/org/freedesktop/FileManager1".into(),
                    "org.freedesktop.FileManager1.ShowItems".into(),
                    uri.into(),
                    "string:".into(),
                ],
            ))
        }
        _ => Err("File reveal is not available on this platform".into()),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    const PDF: &[u8] = include_bytes!("../../tests/data/form.pdf");

    /// The settings file is untrusted: pinned tabs are read as text paths only, each once, and
    /// no more than the cap however many it lists.
    #[test]
    fn pinned_tabs_are_read_leniently_and_capped() {
        let v = serde_json::json!(["/a.pdf", 7, null, "", "/a.pdf", { "path": "/x.pdf" }, "/b.pdf"]);
        assert_eq!(pinned_from_json(&v), [pin_key("/a.pdf"), pin_key("/b.pdf")]);
        assert_eq!(
            pinned_from_json(&serde_json::json!(["report.pdf"])),
            [pin_key("report.pdf")],
            "relative entries use the same normalization as an opened document"
        );
        assert!(pinned_from_json(&serde_json::json!("/a.pdf")).is_empty(), "not a list");
        assert!(pinned_from_json(&serde_json::Value::Null).is_empty());
        let many: Vec<String> = (0..1000).map(|i| format!("/f{i}.pdf")).collect();
        let read = pinned_from_json(&serde_json::json!(many));
        assert_eq!(read.len(), MAX_PINNED);
        assert_eq!(read.first().map(String::as_str), Some("/f0.pdf"));
    }

    #[test]
    fn confirmed_close_uses_id_and_cancel_keeps_history_empty() {
        let mut app = crate::PdfCraftApp::new();
        app.open_bytes("first.pdf", Some("/tmp/first.pdf".into()), PDF.to_vec()).unwrap();
        let first = app.views[0].id;
        app.open_bytes("second.pdf", None, PDF.to_vec()).unwrap();
        app.session.apply(first, pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }).unwrap();
        app.tab_action(Action::Close(first));
        assert_eq!(app.views.len(), 2);
        assert!(app.closed_tabs.is_empty());
        app.resolve_close(&egui::Context::default(), None);
        assert!(app.closed_tabs.is_empty());
        app.tab_action(Action::Close(first));
        app.resolve_close(&egui::Context::default(), Some(false));
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.closed_tabs, ["/tmp/first.pdf"]);
        assert_eq!(app.active, Some(0));
        app.close_tab(0);
        assert_eq!(app.closed_tabs.len(), 1, "pathless tabs are not recorded");
    }

    #[test]
    fn reopen_reads_saved_file_and_history_is_bounded() {
        use std::io::Write as _;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("pdfcraft-tab-{}-{nonce}-年度 report.pdf", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        file.write_all(PDF).unwrap();
        drop(file);
        let mut app = crate::PdfCraftApp::new();
        let path_text = path.to_str().unwrap();
        for _ in 0..12 {
            app.open_path(path_text);
            app.close_tab(0);
        }
        assert_eq!(app.closed_tabs.len(), 10);
        app.tab_action(Action::Reopen);
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
        let mut app = crate::PdfCraftApp::new();
        app.open_path(text);
        app.close_tab(0);
        std::fs::remove_file(&path).unwrap();
        app.tab_action(Action::Reopen);
        assert!(app.views.is_empty());
        assert_eq!(app.closed_tabs, [text]);
        std::fs::write(&path, PDF).unwrap();
        app.tab_action(Action::Reopen);
        assert_eq!(app.views.len(), 1);
        assert!(app.closed_tabs.is_empty());
        app.closed_tabs.push(text.into());
        app.tab_action(Action::Reopen);
        assert_eq!(app.views.len(), 1, "an already-open file is selected, not duplicated");
        assert!(app.closed_tabs.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unavailable_file_manager_falls_back_safely_and_reports_failure() {
        let path = std::path::Path::new("/tmp/年度 & reports/file.pdf");
        let mut calls = Vec::new();
        reveal_via("linux", path, |program, args| {
            calls.push((program.to_string(), args.to_vec()));
            if program == "dbus-send" { Err("FileManager1 unavailable".into()) } else { Ok(true) }
        })
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], ("xdg-open".into(), vec![std::ffi::OsString::from("/tmp/年度 & reports")]));
        assert!(reveal_via("linux", path, |_, _| Ok(false)).is_err());
        assert!(reveal_via("linux", path, |_, _| Err("command missing".into())).is_err());
    }

    /// #2579 (review of #2625): the `/select,"…"` argument reaches the process exactly as written.
    /// `cmd /c echo` prints its command line back; with Rust's own quoting (`args`) it would come
    /// back wrapped in quotes with the inner ones escaped, which Explorer reads as no selection.
    #[cfg(windows)]
    #[test]
    fn the_reveal_argument_reaches_the_process_as_written() {
        let (_, args) = reveal_command("windows", std::path::Path::new(r"C:\Reports\annual report.pdf")).unwrap();
        let mut echo: Vec<std::ffi::OsString> = ["/d", "/c", "echo"].map(std::ffi::OsString::from).to_vec();
        echo.extend(args);
        let out = reveal_process("cmd", &echo).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), r#"/select,"C:\Reports\annual report.pdf""#);
    }

    #[test]
    fn reveal_arguments_preserve_spaces_and_unicode() {
        let path = std::path::Path::new("/tmp/年度 report.pdf");
        let (exe, args) = reveal_command("macos", path).unwrap();
        assert_eq!(exe, "open");
        assert_eq!(args, [std::ffi::OsString::from("-R"), path.as_os_str().into()]);
        let (exe, args) = reveal_command("windows", path).unwrap();
        assert_eq!(exe, "explorer.exe");
        assert_eq!(args, [std::ffi::OsString::from("/select,\"/tmp/年度 report.pdf\"")]);
        // #2579: the path is quoted after the comma, never the whole argument, which Explorer
        // reads as no selection and answers by opening Documents.
        let special = std::path::Path::new(r"C:\Reports\年度 report, #1 & (draft).pdf");
        let (_, windows) = reveal_command("windows", special).unwrap();
        assert_eq!(windows, [std::ffi::OsString::from(r#"/select,"C:\Reports\年度 report, #1 & (draft).pdf""#)]);
        assert!(reveal_command("windows", std::path::Path::new(r#"C:\a" b.pdf"#)).is_err(), "a quote can't end the path early");
        let (exe, args) = reveal_command("linux", path).unwrap();
        assert_eq!(exe, "dbus-send");
        assert_eq!(args[6], "array:string:file:///tmp/%E5%B9%B4%E5%BA%A6%20report.pdf");
        let (_, args) = reveal_command("linux", std::path::Path::new("/tmp/年度 report,#&'$.pdf")).unwrap();
        assert_eq!(args[6], "array:string:file:///tmp/%E5%B9%B4%E5%BA%A6%20report%2C%23%26%27%24.pdf");
        assert_eq!(args[5], "org.freedesktop.FileManager1.ShowItems");
        assert_eq!(args[7], "string:");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod ui_tests {
    use crate::PdfCraftApp;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    #[test]
    fn inactive_tab_menu_closes_target_and_disables_unavailable_actions() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            for name in ["first.pdf", "second.pdf"] {
                app.open_bytes(name, None, include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
            }
            app
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
