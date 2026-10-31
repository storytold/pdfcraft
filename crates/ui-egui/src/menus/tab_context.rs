//! PDF-tab actions. Closed history contains paths only, never bytes or passwords.
use pdfcraft_engine::DocId;

pub(crate) enum Action {
    Close(DocId),
    Reopen,
    Reveal(DocId),
}

impl crate::PdfCraftApp {
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
