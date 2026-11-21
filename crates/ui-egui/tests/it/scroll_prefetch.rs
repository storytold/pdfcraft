//! Continuous view renders the pages ahead of the scroll before they come into view.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

const PAGES: usize = 20;

/// Twenty empty 300×400 pt pages.
fn pages_pdf() -> Vec<u8> {
    let kids: Vec<String> = (0..PAGES).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {PAGES} /MediaBox [0 0 300 400] >> endobj\n",
        kids.join(" ")
    );
    for i in 0..PAGES {
        pdf.push_str(&format!("{} 0 obj << /Type /Page /Parent 2 0 R >> endobj\n", 3 + i));
    }
    pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF");
    pdf.into_bytes()
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, pages_pdf()).expect("opens");
        for (k, v) in [("left", "closed"), ("panel", "none"), ("layout", "continuous"), ("zoom", "100")] {
            app.set_option(k, v).unwrap();
        }
        app
    });
    settle(&mut h);
    h
}

/// Runs frames until no render is queued.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    h.run_steps(4);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while h.state().render_pending() {
        assert!(std::time::Instant::now() < deadline, "renders settle");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

/// Goes to a page (1-based, as the page box takes it) and waits for its renders.
fn go_to(h: &mut Harness<'static, PdfCraftApp>, page: usize) {
    h.state_mut().set_option("page", &page.to_string()).unwrap();
    settle(h);
}

fn shown(h: &Harness<'static, PdfCraftApp>) -> Vec<usize> {
    (0..PAGES).filter(|&p| h.state().views[0].page_screen_rect(p).is_some()).collect()
}

#[test]
fn continuous_view_prefetches_the_pages_ahead_of_the_scroll() {
    let mut h = harness();
    go_to(&mut h, 14);
    let shown_down = shown(&h);
    let last = *shown_down.last().expect("pages on screen");
    let cached = h.state().views[0].cached_pages_debug();
    // The page just past the window is within the 400 pt band every frame asks for; the one
    // after it is past the band, and only the prefetch asks for it.
    for page in last + 1..=last + 2 {
        assert!(cached.contains(&page), "page {page} renders ahead of a scroll down: shown {shown_down:?}, cached {cached:?}");
    }

    // Scrolling back up prefetches above the window instead.
    go_to(&mut h, 9);
    let shown_up = shown(&h);
    let first = *shown_up.first().expect("pages on screen");
    let cached = h.state().views[0].cached_pages_debug();
    for page in first.saturating_sub(2)..first {
        assert!(cached.contains(&page), "page {page} renders ahead of a scroll up: shown {shown_up:?}, cached {cached:?}");
    }
}
