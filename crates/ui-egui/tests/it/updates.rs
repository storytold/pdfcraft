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

#[test]
fn the_check_at_start_offers_the_prompt_and_later_dismisses_it() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_update_source(source(Ok(Some("v99.0.0"))));
        app.set_startup_update_check(true);
        app
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label_contains("is available").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the startup prompt");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("LATER").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the prompt");
}
