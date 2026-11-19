//! Native launch arguments and per-file view overrides (#1354).
use craft::pages::PageDisplay;
use pdfcraft_ui_egui::PdfCraftApp;

#[derive(Default)]
pub(super) struct Arguments {
    pub files: Vec<String>,
    pub options: Vec<(String, String)>,
    /// `--control <PORT>`: the loopback port of the control channel; `0` picks a free one.
    pub control_port: Option<String>,
    /// `--control-token <64-HEX>`; never printed.
    pub control_token: Option<String>,
    /// `--control-token-file <PATH>`: a file that holds the token.
    pub control_token_file: Option<String>,
    pub create_images: bool,
    /// `--combine`: the files go to the Combine files tab.
    pub combine: bool,
    pub new_window: bool,
    pub two_paged: bool,
    pub version: bool,
    pub help: bool,
}

impl Arguments {
    /// Whether the files may go to a running PdfCraft. A launch with a control port keeps its own
    /// window: the channel it was asked to open belongs to this process.
    pub fn may_hand_off(&self) -> bool {
        !self.new_window && !self.files.is_empty() && self.options.is_empty() && self.control_port.is_none() && !self.create_images && !self.two_paged
    }

    pub fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--version" => {
                    parsed.version = true;
                    break;
                }
                "-h" | "--help" => {
                    parsed.help = true;
                    break;
                }
                "--control" => parsed.control_port = args.next(),
                "--control-token" => parsed.control_token = args.next(),
                "--control-token-file" => parsed.control_token_file = args.next(),
                "--create-images" => parsed.create_images = true,
                // File Explorer's "Combine files in PdfCraft" (#585): the files go to the Combine
                // files tab, of the running PdfCraft when there is one.
                "--combine" => parsed.combine = true,
                "--new-window" => parsed.new_window = true,
                "--two-paged" => parsed.two_paged = true,
                flag if flag.starts_with("--two-paged=") => {
                    // It has no value: do not consume the following filename on malformed use.
                    log::warn!("{flag}: use --two-paged without a value");
                }
                flag if flag.starts_with("--") => {
                    let value = args.next().unwrap_or_default();
                    parsed.options.push((flag.trim_start_matches("--").to_string(), value));
                }
                _ => parsed.files.push(craft::platform::default_app::path_from_arg(a)),
            }
        }
        parsed
    }
}

pub(super) fn open_files(app: &mut PdfCraftApp, files: &[String], two_paged: bool) {
    for path in files {
        if two_paged {
            // The per-file override wins over initial view without changing saved defaults.
            app.open_path_with_display(path, PageDisplay::TwoUp, false);
        } else {
            app.open_path(path);
        }
    }
}

/// An app on an in-memory host whose jobs run inline, so an open has finished when it returns.
#[cfg(test)]
pub(crate) fn test_app() -> PdfCraftApp {
    use craft::host::{AppInfo, Host, InlineJobs, Wake};
    use craft::platform::clipboard::{Clipboard, SystemClipboard};
    use craft::platform::open::FileIntake;
    use craft::platform::watch::FileWatch;
    let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(std::sync::Arc::new(InlineJobs)).build();
    let clipboard = Clipboard::new(SystemClipboard::new());
    let services = pdfcraft_ui_egui::AppServices::new(
        Wake::none(),
        clipboard,
        None,
        None,
        FileIntake::new(Wake::none()).0,
        FileWatch::new(Wake::none()).inline(),
    );
    PdfCraftApp::with_host(std::sync::Arc::new(host), services)
}

/// `app` in a window: an open it starts finishes on the window's next frame, as at launch.
#[cfg(test)]
pub(crate) fn shown(app: PdfCraftApp) -> egui_kittest::Harness<'static, PdfCraftApp> {
    egui_kittest::Harness::builder().build_eframe(move |cc| {
        crate::tests::install_runtime(&cc.egui_ctx);
        app
    })
}

/// Opens `files` as a launch does, then runs the frames that take the opened documents.
#[cfg(test)]
pub(crate) fn launch(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, files: &[String], two_paged: bool) {
    open_files(h.state_mut(), files, two_paged);
    h.run_steps(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The display the saved settings name as the default: what a restart would open with.
    fn saved_display(app: &PdfCraftApp) -> PageDisplay {
        let token = app.settings_snapshot().layout.default_layout;
        token.as_deref().and_then(craft::pages::parse_display_token).map(|(display, _)| display).unwrap_or_default()
    }

    fn parse(args: &[&str]) -> Arguments {
        Arguments::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn standalone_flag_keeps_files_in_either_position() {
        for args in
            [["--two-paged", "first.pdf", "second.pdf"], ["first.pdf", "--two-paged", "second.pdf"], ["first.pdf", "second.pdf", "--two-paged"]]
        {
            let parsed = parse(&args);
            assert!(parsed.two_paged);
            assert_eq!(parsed.files, ["first.pdf", "second.pdf"]);
            assert!(parsed.options.is_empty());
            assert!(!parsed.may_hand_off());
        }
        let empty = parse(&["--two-paged"]);
        assert!(empty.two_paged && empty.files.is_empty());
        let normal = parse(&["first.pdf", "second.pdf"]);
        assert!(!normal.two_paged);
        assert!(normal.may_hand_off());
        for args in [
            &["--new-window", "first.pdf"][..],
            &["--layout", "two-page", "first.pdf"],
            &["--control", "0", "first.pdf"],
            &["--create-images", "first.png"],
        ] {
            assert!(!parse(args).may_hand_off());
        }
    }

    #[test]
    fn existing_layout_options_are_preserved_in_either_order() {
        for args in [["--two-paged", "--layout", "single", "file.pdf"], ["--layout", "single", "--two-paged", "file.pdf"]] {
            let parsed = parse(&args);
            assert!(parsed.two_paged);
            assert_eq!(parsed.files, ["file.pdf"]);
            assert_eq!(parsed.options, [("layout".into(), "single".into())]);
        }
        let parsed = parse(&["--two-paged", "--default-layout", "two-up", "file.pdf"]);
        assert_eq!(parsed.options, [("default-layout".into(), "two-up".into())]);
    }

    #[test]
    fn newer_options_and_version_are_preserved() {
        let parsed = parse(&["--two-paged", "--new-window", "--grid-zoom", "150", "--default-mode", "read", "file:///tmp/a%20b.pdf"]);
        assert!(parsed.two_paged && parsed.new_window);
        assert_eq!(parsed.files, ["/tmp/a b.pdf"]);
        assert_eq!(parsed.options, [("grid-zoom".into(), "150".into()), ("default-mode".into(), "read".into())]);
        assert!(!parsed.may_hand_off());
        let version = parse(&["--two-paged", "--version", "ignored.pdf"]);
        assert!(version.version && version.files.is_empty());
    }

    #[test]
    fn help_keeps_the_headless_early_exit() {
        for flag in ["-h", "--help"] {
            let args = parse(&["--two-paged", flag, "ignored.pdf"]);
            assert!(args.help && args.files.is_empty());
        }
    }

    #[test]
    fn malformed_boolean_does_not_eat_a_file_or_option() {
        let args = parse(&["--two-paged=false", "first.pdf", "--layout", "single", "second.pdf"]);
        assert!(!args.two_paged);
        assert_eq!(args.files, ["first.pdf", "second.pdf"]);
        assert_eq!(args.options, [("layout".into(), "single".into())]);
    }

    #[test]
    fn startup_override_is_per_file_and_does_not_persist() {
        use std::io::Write as _;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("pdfcraft-two-paged-{}-{nonce}.pdf", std::process::id()));
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
        file.write_all(include_bytes!("../../../crates/ui-egui/tests/data/form.pdf")).unwrap();
        drop(file);
        let second_path = path.with_extension("second.pdf");
        std::fs::copy(&path, &second_path).unwrap();
        let text = path.to_str().unwrap();
        let second = second_path.to_str().unwrap();
        for enabled in [false, true] {
            let mut app = test_app();
            app.set_option(&egui::Context::default(), "default-layout", "single").unwrap();
            app.open_bytes(&egui::Context::default(), "restored.pdf", None, include_bytes!("../../../crates/ui-egui/tests/data/form.pdf").to_vec())
                .unwrap();
            let restored = app.views[0].id;
            let mut h = shown(app);
            launch(&mut h, &[text.into(), second.into()], enabled);
            let app = h.state_mut();
            assert_eq!(app.views.len(), 3);
            assert_eq!(app.views[0].id, restored);
            assert_eq!(app.views[0].view.display, PageDisplay::Single);
            for view in app.views.iter().skip(1) {
                assert_eq!(view.view.display, if enabled { PageDisplay::TwoUp } else { PageDisplay::Single });
                assert!(!view.view.continuous);
            }
            // The launch override never reaches the saved settings.
            assert_eq!(saved_display(app), PageDisplay::Single);
            // Existing --layout applies after the flag to the active document only.
            if enabled {
                app.set_launch_layout("continuous").unwrap();
            } else {
                app.set_option(&egui::Context::default(), "layout", "continuous").unwrap();
            }
            assert_eq!(app.views[2].view.display, PageDisplay::Single);
            assert!(app.views[2].view.continuous);
            app.set_option(&egui::Context::default(), "default-layout", "two-up").unwrap();
            assert_eq!(app.view_defaults.display, PageDisplay::TwoUp);
            assert!(app.view_defaults.continuous);
            assert_eq!(saved_display(app), PageDisplay::TwoUp);
            launch(&mut h, &[], enabled);
            assert_eq!(h.state().views.len(), 3);
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(second_path).unwrap();
    }

    #[test]
    fn explicit_launch_layout_preserves_argument_order_and_active_file() {
        let path = std::env::temp_dir().join(format!("pdfcraft-explicit-layout-{}.pdf", std::process::id()));
        std::fs::write(&path, include_bytes!("../../../crates/ui-egui/tests/data/form.pdf")).unwrap();
        let second_path = path.with_extension("second.pdf");
        std::fs::copy(&path, &second_path).unwrap();
        let names = [path.to_str().unwrap(), second_path.to_str().unwrap()];
        for prefix in [["--two-paged", "--layout", "single"], ["--layout", "single", "--two-paged"]] {
            for count in [1, 2] {
                let args = Arguments::parse(prefix.into_iter().chain(names.iter().take(count).copied()).map(str::to_owned));
                let mut app = test_app();
                app.set_option(&egui::Context::default(), "default-layout", "continuous").unwrap();
                app.open_bytes(
                    &egui::Context::default(),
                    "restored.pdf",
                    None,
                    include_bytes!("../../../crates/ui-egui/tests/data/form.pdf").to_vec(),
                )
                .unwrap();
                let mut h = shown(app);
                launch(&mut h, &args.files, args.two_paged);
                let app = h.state_mut();
                for (key, value) in args.options {
                    assert_eq!(key, "layout");
                    app.set_launch_layout(&value).unwrap();
                }
                assert_eq!(app.active, Some(count));
                assert!(app.views[0].view.continuous);
                for view in app.views.iter().skip(1).take(count - 1) {
                    assert_eq!(view.view.display, PageDisplay::TwoUp);
                }
                assert_eq!(app.views[count].view.display, PageDisplay::Single);
                assert!(!app.views[count].view.continuous);
                assert!(app.view_defaults.continuous);
            }
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(second_path).unwrap();
    }
}

#[cfg(test)]
mod repeated_file_tests {
    use super::*;

    #[test]
    fn a_repeated_file_reuses_its_tab_and_receives_the_launch_override() {
        let path = std::env::temp_dir().join(format!("pdfcraft-repeated-startup-{}.pdf", std::process::id()));
        std::fs::write(&path, include_bytes!("../../../crates/ui-egui/tests/data/form.pdf")).unwrap();
        let name = path.to_str().unwrap().to_string();
        let mut app = test_app();
        app.set_option(&egui::Context::default(), "default-layout", "continuous").unwrap();
        let mut h = shown(app);
        launch(&mut h, std::slice::from_ref(&name), false);
        let original = h.state().views[0].id;
        launch(&mut h, &[name.clone(), name], true);
        let app = h.state_mut();
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.views[0].id, original);
        assert_eq!(app.views[0].view.display, PageDisplay::TwoUp);
        assert!(!app.views[0].view.continuous);
        app.set_launch_layout("single").unwrap();
        assert_eq!(app.views[0].view.display, PageDisplay::Single);
        assert!(app.view_defaults.continuous);
        std::fs::remove_file(path).unwrap();
    }
}
