//! Help ▸ Check for updates (issue #28), with stand-in release sources (no network).

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::UpdateSource;

fn release(tag: &str) -> craft_update::Release {
    craft_update::Release {
        tag: tag.to_string(),
        version: craft_update::parse_version(tag),
        html_url: format!("https://github.com/storytold/pdfcraft/releases/tag/{tag}"),
        ..Default::default()
    }
}

/// `Ok(Some(tag))` is a newer release, `Ok(None)` is up to date.
fn source(answer: Result<Option<&str>, &str>) -> UpdateSource {
    let answer = answer.map(|tag| tag.map(release)).map_err(str::to_string);
    Arc::new(move || answer.clone())
}

fn harness(answer: Result<Option<&str>, &str>) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_update_source(source(answer));
        app
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

#[test]
fn a_newer_release_is_offered_for_download() {
    let mut h = harness(Ok(Some("v99.0.0")));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
    h.get_by_label("DOWNLOAD");
    h.get_by_label("LATER").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the dialog");
}

#[test]
fn an_up_to_date_or_failed_check_says_so() {
    let mut h = harness(Ok(None));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("is the newest version available");
    assert!(h.query_by_label("Download").is_none());

    let mut h = harness(Err("couldn't reach GitHub"));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("Could not check for updates.");
    h.get_by_label_contains("couldn't reach GitHub");
}

#[test]
fn nothing_is_asked_until_the_user_checks() {
    // The check at start is opt-in (the desktop app turns it on): the source is only called from the command.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Settings from a build that had the startup option are ignored.
        app.restore(r#"{"check_updates_at_start": true}"#);
        let counted = counted.clone();
        app.set_update_source(Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(release("v99.0.0")))
        }));
        app
    });
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(h.query_by_label_contains("is available").is_none());
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
}

/// The desktop app's start, with `settings` restored, and the count of what the source was asked.
fn started_with(settings: &'static str) -> (Harness<'static, PdfCraftApp>, Arc<std::sync::atomic::AtomicUsize>) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_startup_update_check(true);
        app.restore(settings);
        let counted = counted.clone();
        app.set_update_source(Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(release("v99.0.0")))
        }));
        app
    });
    (h, calls)
}

/// Run frames for `time`, or until the source has been asked. The check at start shows nothing
/// while it runs, so `settle` can't wait for it.
fn run_until_asked(h: &mut Harness<'static, PdfCraftApp>, calls: &std::sync::atomic::AtomicUsize, time: std::time::Duration) {
    let deadline = std::time::Instant::now() + time;
    while calls.load(std::sync::atomic::Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Whether this executable uses the internal-build startup policy.
fn labelled_build() -> bool {
    pdfcraft_engine::build_info::LABEL.is_some_and(|s| !s.trim().is_empty())
}

/// An offered dialog proves the worker result was consumed, unlike a source-call counter.
fn wait_for_offer(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label_contains("is available").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update offer");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The preference and compiled policy both prevent automatic calls; manual checks still work.
#[test]
fn the_preference_keeps_the_check_at_start_from_reaching_the_network() {
    use std::sync::atomic::Ordering::SeqCst;
    assert!(PdfCraftApp::new().update_check_at_start, "on by default");

    // Ordinary builds ask with the preference on; labelled builds suppress startup checks.
    let (mut h, calls) = started_with("{}");
    let automatic = usize::from(!labelled_build());
    let wait = if automatic == 1 { std::time::Duration::from_secs(30) } else { std::time::Duration::from_millis(500) };
    run_until_asked(&mut h, &calls, wait);
    assert_eq!(calls.load(SeqCst), automatic, "startup obeys the compiled policy");

    let (mut h, calls) = started_with(r#"{"update_check_at_start": false}"#);
    run_until_asked(&mut h, &calls, std::time::Duration::from_millis(500));
    assert_eq!(calls.load(SeqCst), 0, "nothing is asked at start");

    let mut restarted = PdfCraftApp::new();
    restarted.restore(&h.state().persist());
    assert!(!restarted.update_check_at_start, "the choice is kept");

    // Help ▸ Check for updates still asks.
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(SeqCst), 1);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
}

/// The setting is about starting: turning it on asks nothing until the next start.
#[test]
fn turning_the_check_at_start_on_waits_for_the_next_start() {
    use std::sync::atomic::Ordering::SeqCst;
    let (mut h, calls) = started_with(r#"{"update_check_at_start": false}"#);
    h.run_steps(3);
    h.state_mut().set_option("update_check_at_start", "true").unwrap();
    run_until_asked(&mut h, &calls, std::time::Duration::from_millis(500));
    assert_eq!(calls.load(SeqCst), 0, "nothing is asked this session");
}

#[test]
fn preferences_turn_the_check_at_start_off() {
    let (mut h, _calls) = started_with(r#"{"update_check_at_start": true}"#);
    h.state_mut().execute("app.preferences");
    h.run_steps(3);
    h.get_by_label("Check for updates when PdfCraft starts").click();
    h.run_steps(3);
    assert!(!h.state().update_check_at_start);
    // The control channel's option, as an agent sets it.
    h.state_mut().set_option("update_check_at_start", "true").unwrap();
    assert!(h.state().update_check_at_start);
    assert!(h.state_mut().set_option("update_check_at_start", "maybe").is_err());
}

#[test]
fn settings_from_the_old_opt_in_leave_the_check_at_start_on() {
    let mut app = PdfCraftApp::new();
    app.restore(r#"{"check_updates_at_start": false}"#);
    assert!(app.update_check_at_start, "a key of the removed option decides nothing");
}

/// Ordinary startup offers a release; labelled builds offer it only through the manual command.
#[test]
fn the_check_at_start_offers_the_prompt_and_later_dismisses_it() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_update_source(source(Ok(Some("v99.0.0"))));
        app.set_startup_update_check(true);
        app
    });
    if labelled_build() {
        h.run_steps(20);
        assert!(h.query_by_label_contains("is available").is_none(), "labelled builds never offer at startup");
        h.state_mut().execute("help.check_updates");
    }
    wait_for_offer(&mut h);
    h.get_by_label("LATER").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the prompt");
}

/// Complete a deliberately held startup request before requiring a fresh manual request.
#[test]
fn compiled_build_policy_keeps_manual_checks_available() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let labelled = labelled_build();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let (release_startup, startup_ready) = std::sync::mpsc::channel::<()>();
    let startup_ready = Arc::new(std::sync::Mutex::new(startup_ready));
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let calls = observed.clone();
        let startup_ready = startup_ready.clone();
        app.set_update_source(Arc::new(move || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if !labelled && call == 0 {
                startup_ready.lock().unwrap().recv_timeout(std::time::Duration::from_secs(30)).map_err(|e| e.to_string())?;
                Ok(Some(release("v99.0.0")))
            } else {
                Ok(None)
            }
        }));
        app.set_startup_update_check(true);
        app
    });
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let automatic = usize::from(!labelled);
    if !labelled {
        run_until_asked(&mut h, &calls, std::time::Duration::from_secs(30));
        assert!(h.query_by_label_contains("is available").is_none(), "the source started but has not returned");
        release_startup.send(()).unwrap();
        wait_for_offer(&mut h);
        h.get_by_label("LATER").click();
        h.run_steps(3);
    }
    assert_eq!(calls.load(Ordering::SeqCst), automatic, "compiled labels suppress startup network requests");
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), automatic + 1, "manual checks remain available");
    h.get_by_label_contains("is the newest version available");
}

#[test]
fn compiled_build_label_names_the_actual_viewport() {
    let mut h = harness(Ok(None));
    h.run_steps(3);
    let expected = match pdfcraft_engine::build_info::LABEL.map(str::trim).filter(|s| !s.is_empty()) {
        Some(label) => format!("PdfCraft ({label})"),
        None => "PdfCraft".into(),
    };
    assert_eq!(h.state().window_title, expected);
    assert_eq!(h.state().page_title, expected);
    let bytes = h.state().session.create_blank(200.0, 300.0, 1).unwrap().as_ref().clone();
    h.state_mut().open_bytes("Synthetic.pdf", None, bytes).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().window_title, format!("Synthetic.pdf — {expected}"));
    assert_eq!(h.state().page_title, format!("Synthetic.pdf — {expected}"));
}
