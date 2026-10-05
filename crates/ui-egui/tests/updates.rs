//! Help ▸ Check for updates (issue #28), with stand-in release sources (no network).

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use printcraft_ui_egui::PrintCraftApp;
use printcraft_ui_egui::updates::{Release, UpdateSource, is_newer};

fn source(answer: Result<&str, &str>) -> UpdateSource {
    let answer = answer.map(str::to_string).map_err(str::to_string);
    Arc::new(move || answer.clone().map(|v| Release { url: format!("https://github.com/storytold/printcraft/releases/tag/{v}"), version: v }))
}

fn harness(answer: Result<&str, &str>, at_start: bool) -> Harness<'static, PrintCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PrintCraftApp::new();
        app.update_source = Some(source(answer));
        app.check_updates_at_start = at_start;
        app
    })
}

/// Run frames until the background check has reported (or give up).
fn settle(h: &mut Harness<'static, PrintCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if h.query_by_label_contains("Checking for a newer version").is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn versions_compare_by_number() {
    assert!(is_newer("v0.2.0", "0.1.1"));
    assert!(is_newer("v0.1.10", "0.1.9"));
    assert!(is_newer("1", "0.9.9"));
    assert!(!is_newer("v0.1.1", "0.1.1"));
    assert!(!is_newer("v0.1.0", "0.1.1"));
    assert!(!is_newer("v0.1.1-beta.2", "0.1.1"), "suffixes are ignored");
    assert!(!is_newer("nightly", "0.1.1"), "a tag that isn't a version is never newer");
    assert!(!is_newer("v1.2.3.4", "0.1.1"));
    assert!(!is_newer("v99999999999999999999.0.0", "0.1.1"), "out of range");
}

#[test]
fn a_newer_release_is_offered_for_download() {
    let mut h = harness(Ok("v99.0.0"), false);
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("PrintCraft 99.0.0 is available");
    h.get_by_label("Download");
    h.get_by_label("Later").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the dialog");
}

#[test]
fn an_up_to_date_or_failed_check_says_so() {
    let mut h = harness(Ok(concat!("v", env!("CARGO_PKG_VERSION"))), false);
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("is up to date");
    assert!(h.query_by_label("Download").is_none());

    let mut h = harness(Err("couldn't reach GitHub"), false);
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("Couldn't check for updates: couldn't reach GitHub");
}

#[test]
fn the_check_at_start_is_opt_in_and_quiet_unless_there_is_news() {
    // Off (the default): nothing is asked.
    let mut h = harness(Ok("v99.0.0"), false);
    settle(&mut h);
    assert!(h.query_by_label_contains("is available").is_none());
    // On, with a newer release: the dialog appears by itself.
    let mut h = harness(Ok("v99.0.0"), true);
    settle(&mut h);
    h.get_by_label_contains("PrintCraft 99.0.0 is available");
    // On, up to date or failing: silent.
    for answer in [Ok(concat!("v", env!("CARGO_PKG_VERSION"))), Err("offline")] {
        let mut h = harness(answer, true);
        settle(&mut h);
        for message in ["is up to date", "Couldn't check for updates", "is available"] {
            assert!(h.query_by_label_contains(message).is_none(), "{answer:?}: {message}");
        }
    }
    // The preference is remembered.
    let mut app = PrintCraftApp::new();
    app.check_updates_at_start = true;
    let mut restored = PrintCraftApp::new();
    restored.restore(&app.persist());
    assert!(restored.check_updates_at_start);
    restored.restore("{}");
    assert!(restored.check_updates_at_start, "missing keys change nothing");
}
