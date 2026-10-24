//! State-based waits for native preparation, which completes between UI frames.
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// Step frames until `ready` holds or 30 seconds pass, and say whether it held. For background work
/// (a worker-thread open, page text arriving) that a test then asserts on itself: a frame count is
/// not a time, and a busy runner may not have finished the work within one. Sleeps briefly between
/// frames so the worker threads get the CPU.
#[must_use]
pub fn step_until(h: &mut Harness<'static, PdfCraftApp>, mut ready: impl FnMut(&mut Harness<'static, PdfCraftApp>) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        if ready(h) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Step frames until nothing is waiting to be rendered, and fail the test if that takes over 30
/// seconds: what a test checks next is on the rendered page.
pub fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let settled = step_until(h, |h| {
        h.run_steps(1);
        !h.state().render_pending()
    });
    assert!(settled, "pages rendered");
}

pub fn wait_until(h: &mut Harness<'static, PdfCraftApp>, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    let held = step_until(h, |h| ready(h.state()));
    assert!(
        held,
        "timed out waiting for {what}: views={}, active={:?}, password={:?}, toast={:?}",
        h.state().views.len(),
        h.state().active,
        h.state().password_prompt.as_ref().map(|p| (&p.name, &p.error)),
        h.state().toast
    );
}
