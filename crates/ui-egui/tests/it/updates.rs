//! Help ▸ Check for updates (issue #28), with stand-in release sources (no network): the source is
//! the `CheckSource` given in the services the app is built with.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use craft::host::Host;
use craft::ui::dialog::StackDialog;
use craft::update::stack_dialogs::{CheckChoice, UpdateCheckDialog, UpdatePromptDialog};
use craft::update::updater::CheckSource;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{Edit, FieldValue};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::import_json;

use crate::control::{info, services_asking};
use crate::support::AppHarness as _;

fn release(tag: &str) -> craft::update::Release {
    craft::update::Release {
        tag: tag.to_string(),
        version: craft::update::parse_version(tag),
        html_url: format!("https://github.com/storytold/pdfcraft/releases/tag/{tag}"),
        ..Default::default()
    }
}

/// `Ok(Some(tag))` is a newer release, `Ok(None)` is up to date.
fn source(answer: Result<Option<&str>, &str>) -> CheckSource {
    let answer = answer.map(|tag| tag.map(release)).map_err(craft::update::CheckError::from);
    Arc::new(move || answer.clone())
}

/// A source that counts the times it is asked and always offers `v99.0.0`.
fn counting_source(calls: &Arc<AtomicUsize>) -> CheckSource {
    let calls = calls.clone();
    Arc::new(move || {
        calls.fetch_add(1, SeqCst);
        Ok(Some(release("v99.0.0")))
    })
}

/// An app on the default host whose update check asks `source`.
fn app_asking(source: CheckSource) -> PdfCraftApp {
    let host = Arc::new(Host::builder(info()).build());
    let mut app = PdfCraftApp::with_host(host, services_asking(Some(source)));
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app
}

fn harness(answer: Result<Option<&str>, &str>) -> Harness<'static, PdfCraftApp> {
    let source = source(answer);
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        app_asking(source)
    })
}

/// Run frames until the background check has reported. By what the dialog shows, with the
/// same 30-second deadline as `support::wait_until`, not for a fixed second: under load the
/// check's thread can take longer (#1277).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.run_steps(2);
        if h.query_by_label_contains("Checking for updates").is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update check");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

/// Whether an update dialog is on the host: the check's result or the prompt of a found release.
fn update_dialog_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().dialogs.is_open::<UpdateCheckDialog>() || h.state().dialogs.is_open::<UpdatePromptDialog>()
}

#[test]
fn a_newer_release_is_offered_for_download() {
    #[cfg(target_os = "linux")]
    {
        const FIXTURE: &str = "PDFCRAFT_TEST_UPDATE_DOWNLOAD_FIXTURE";
        if craft::update::self_update::is_self_updating(&pdfcraft_ui_egui::UPDATE_APP) {
            assert!(std::env::var_os(FIXTURE).is_none(), "the fixture must be detected as a Cargo build");
            // nextest's extracted archive can omit Cargo's metadata. Give the child a
            // private build directory with the marker the updater uses to identify it.
            let current = std::env::current_exe().unwrap();
            let build = tempfile::tempdir_in(current.parent().unwrap()).unwrap();
            std::fs::write(build.path().join(".rustc_info.json"), "{}").unwrap();
            let exe = build.path().join("update-download-test");
            if std::fs::hard_link(&current, &exe).is_err() {
                std::fs::copy(&current, &exe).unwrap();
            }
            let output = std::process::Command::new(exe)
                .args(["--exact", "updates::a_newer_release_is_offered_for_download", "--nocapture"])
                .env(FIXTURE, "1")
                .env_remove(craft::update::self_update::DEV_VAR)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
    }
    let mut h = harness(Ok(Some("v99.0.0")));
    assert!(h.execute("help.check_updates"));
    settle(&mut h);
    assert!(update_dialog_open(&h), "the found release is offered in a dialog");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!update_dialog_open(&h), "Escape closes the dialog");
}

/// The desktop app's start, with `settings` applied, and the count of what the source was asked.
fn started_with(settings: &'static str) -> (Harness<'static, PdfCraftApp>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = counting_source(&calls);
    let h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_asking(source);
            app.set_startup_update_check(true);
            app.apply_settings(&import_json(settings).0);
            app
        }
    });
    (h, calls)
}

/// Run frames for `time`, or until the source has been asked. The check at start shows nothing
/// while it runs, so `settle` can't wait for it.
fn run_until_asked(h: &mut Harness<'static, PdfCraftApp>, calls: &AtomicUsize, time: std::time::Duration) {
    let deadline = std::time::Instant::now() + time;
    while calls.load(SeqCst) == 0 && std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Whether the settings the app would save have the check at start on; an app that saved no value
/// has the default, which is on.
fn check_at_start_saved(app: &PdfCraftApp) -> bool {
    app.settings_snapshot().layout.update_check_at_start.unwrap_or(true)
}

#[test]
fn the_preference_keeps_the_check_at_start_from_reaching_the_network() {
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(check_at_start_saved(&app_asking(counting_source(&calls))), "on by default");

    // The same start with the preference left on does ask, so a count of none below means
    // the preference held the check back and not that it hadn't run yet.
    let (mut h, calls) = started_with("{}");
    run_until_asked(&mut h, &calls, std::time::Duration::from_secs(30));
    assert_eq!(calls.load(SeqCst), 1, "on, the app asks at start");

    let (mut h, calls) = started_with(r#"{"update_check_at_start": false}"#);
    run_until_asked(&mut h, &calls, std::time::Duration::from_millis(500));
    assert_eq!(calls.load(SeqCst), 0, "nothing is asked at start");

    let mut restarted = app_asking(source(Ok(None)));
    restarted.apply_settings(&h.state().settings_snapshot());
    assert!(!check_at_start_saved(&restarted), "the choice is kept");

    // Help ▸ Check for updates still asks.
    assert!(h.execute("help.check_updates"));
    settle(&mut h);
    assert_eq!(calls.load(SeqCst), 1);
    assert!(update_dialog_open(&h), "the release the source offered is on screen");
}

#[test]
fn settings_from_the_old_opt_in_leave_the_check_at_start_on() {
    let mut app = app_asking(source(Ok(None)));
    app.apply_settings(&import_json(r#"{"check_updates_at_start": false}"#).0);
    assert!(check_at_start_saved(&app), "a key of the removed option decides nothing");
    assert_ne!(app.settings_snapshot().layout.update_check_at_start, Some(false));
}

/// An app with an unsaved change to a form in a new temporary folder, which Save writes without
/// asking where; its update has installed and the check dialog says so. Returns the document's
/// path too.
fn app_with_unsaved_file(test: &str) -> (Harness<'static, PdfCraftApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-updates-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.pdf");
    let bytes = include_bytes!("../data/form.pdf").to_vec();
    std::fs::write(&path, &bytes).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let updater = craft::update::updater::Updater::ready_for_tests(&pdfcraft_ui_egui::UPDATE_APP, "0.1.0", release("v99.0.0"), || Ok(()));
        let host = Arc::new(Host::builder(info()).build());
        let mut app = PdfCraftApp::with_host(host, services_asking(None).with_updater(updater));
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app
    });
    h.run_steps(2);
    let app = h.state_mut();
    app.open_bytes(&egui::Context::default(), "source.pdf", Some(path.to_string_lossy().into_owned()), bytes).unwrap();
    assert!(app.apply_edit(&egui::Context::default(), Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Changed".into()) }));
    assert!(h.execute("help.check_updates"));
    settle(&mut h);
    assert!(h.state().dialogs.is_open::<UpdateCheckDialog>(), "the check's result is on screen");
    (h, path)
}

/// Answers the open check dialog with Restart to Update, as its button does.
fn restart_to_update(h: &mut Harness<'static, PdfCraftApp>) {
    let dialog = h.state_mut().dialogs.get_mut::<UpdateCheckDialog>().expect("the check dialog is open");
    dialog.set_choice(CheckChoice::RestartToUpdate);
    let id = dialog.id();
    let ctx = h.ctx.clone();
    assert!(h.state_mut().dialogs.confirm_by_id(&ctx, id));
    h.run_steps(3);
}

/// Presses the button `label` of the question on screen: the last match, since the question draws
/// over the toolbar's own buttons.
fn press_in_the_question(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.query_all_by_label(label).last().unwrap_or_else(|| panic!("the question has a {label} button")).click();
    h.run_steps(3);
}

fn close_requested(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)))
}

/// A ready update whose Restart to Update meets unsaved work asks about it first, and Cancel keeps
/// the app open with its document still unsaved.
#[test]
fn restart_to_update_with_unsaved_changes_asks_first_and_cancel_keeps_the_app() {
    let (mut h, path) = app_with_unsaved_file("cancel");
    restart_to_update(&mut h);
    assert!(!h.state().dialogs.is_open::<UpdateCheckDialog>(), "the result dialog closes");
    assert!(!h.state().dialogs.open_dialogs().is_empty(), "the unsaved-changes question is on the host");
    press_in_the_question(&mut h, "Cancel");
    assert!(h.state().dialogs.open_dialogs().is_empty(), "Cancel closes the question");
    assert!(!close_requested(&h), "Cancel keeps the app open");
    assert!(h.state().first_dirty().is_some(), "the change is still unsaved");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Saving at the question writes the edited file, and the app then quits.
#[test]
fn saving_at_the_restart_question_writes_the_file_and_quits() {
    let (mut h, path) = app_with_unsaved_file("save");
    restart_to_update(&mut h);
    assert!(!h.state().dialogs.open_dialogs().is_empty(), "the unsaved-changes question is on the host");
    let accept = "Save";
    h.query_all_by_label(accept).last().expect("the question has a Save/Yes button").click();
    let quit = crate::support::step_until(&mut h, |h| close_requested(h));
    assert!(quit, "the app quits once the document is saved");

    let mut reopened = crate::control::new_app();
    reopened.open_bytes(&egui::Context::default(), "source.pdf", Some(path.to_string_lossy().into_owned()), std::fs::read(&path).unwrap()).unwrap();
    let doc = reopened.session.get(reopened.views[0].id).unwrap();
    let name = doc.form.iter().find(|f| f.name == "name").and_then(|f| f.value.first().cloned());
    assert_eq!(name.as_deref(), Some("Changed"), "the change is in the file");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
