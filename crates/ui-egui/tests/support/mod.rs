//! State-based waits for native preparation, which completes between UI frames.
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

pub fn wait_until(h: &mut Harness<'static, PdfCraftApp>, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        if ready(h.state()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: views={}, active={:?}, password={:?}, toast={:?}",
            h.state().views.len(),
            h.state().active,
            h.state().password_prompt.as_ref().map(|p| (&p.name, &p.error)),
            h.state().toast
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
