//! Canvas scrolling tests, verifying that mouse wheel events do not trigger kinetic fling jumps (#4917).

use egui::{Modifiers, MouseWheelUnit, TouchPhase, vec2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// Three 300x400 pt pages in a continuous scroll view.
const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Creates a test harness with stepped frame time and standard continuous layout.
fn harness(step_dt: f32) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(step_dt).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("layout", "continuous").unwrap();
        app.set_option("zoom", "300").unwrap();
        app
    });
    h.run_steps(8);
    h
}

/// Helper getting the screen rect of a page.
fn rect(h: &Harness<'static, PdfCraftApp>, page: usize) -> Option<egui::Rect> {
    h.state().views[0].page_screen_rect(page)
}

/// Measures total vertical scroll movement over `frames` steps.
fn scroll_over(h: &mut Harness<'static, PdfCraftApp>, frames: usize) -> f32 {
    let tops = |h: &Harness<'static, PdfCraftApp>| (0..3).map(|p| rect(h, p).map(|r| r.min.y)).collect::<Vec<_>>();
    let mut moved = 0.0;
    for _ in 0..frames {
        let before = tops(h);
        h.step();
        let after = tops(h);
        let step = before.iter().zip(&after).find_map(|(b, a)| Some(b.as_ref()? - a.as_ref()?));
        moved += step.expect("a page stays on screen from one frame to the next");
    }
    moved
}

/// Rapid discrete mouse wheel steps must never trigger kinetic momentum (#4917).
#[test]
fn fast_mouse_wheel_scrolling_does_not_trigger_kinetic_fling_jumps() {
    let mut h = harness(1.0 / 60.0);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);

    // Send rapid discrete mouse wheel notches (line delta).
    for _ in 0..10 {
        h.event(egui::Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.step();
    }

    // Step 20 frames to allow egui's internal ~0.25s notch easing to finish settling.
    h.run_steps(20);

    // After mouse wheel scrolling ceases, the view must not exhibit any fling glide or drift.
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "mouse wheel must not trigger kinetic fling drift: drifted {drift}");
}

/// A mouse wheel notch immediately stops an active touchpad fling glide (#4917).
#[cfg(not(target_os = "macos"))]
#[test]
fn mouse_wheel_notch_interrupts_active_touchpad_glide() {
    let mut h = harness(1.0 / 60.0);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);

    // Perform a touchpad swipe to enter a gliding state.
    h.event(egui::Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, -25.0),
        phase: TouchPhase::Start,
        modifiers: Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.step();

    for _ in 0..6 {
        h.event(egui::Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -25.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.step();
    }

    h.event(egui::Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, 0.0),
        phase: TouchPhase::End,
        modifiers: Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.step();

    // Verify the page is gliding forward.
    let gliding_delta = scroll_over(&mut h, 3);
    assert!(gliding_delta > 10.0, "touchpad swipe must glide after lift: moved {gliding_delta}");

    // Deliver a discrete wheel notch.
    h.event(egui::Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, -1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.step();
    h.run_steps(2);

    // After the wheel notch, the prior fling glide must be completely cancelled.
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "mouse wheel notch must stop kinetic glide: drifted {drift}");
}
