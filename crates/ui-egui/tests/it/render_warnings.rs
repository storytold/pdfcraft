//! A page the renderer draws only in part or approximately is marked as such in the viewer, not
//! shown as if it were complete (#4340).

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::RenderWarning;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300 × 400 pt page painted in a DeviceN colour whose tint transform `program` (a
/// calculator function taking the two tints) gives the DeviceRGB colour.
pub(crate) fn devicen_pdf(program: &str) -> Vec<u8> {
    let content = "/CS1 cs 0.5 0.5 scn 0 0 150 200 re f";
    format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /ColorSpace << /CS1 [/DeviceN [/Spot1 /Spot2] /DeviceRGB 5 0 R] >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Length {} >> stream\n{program}\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len(),
        program.len()
    )
    .into_bytes()
}

/// The app with `bytes` open, run until every requested raster has arrived.
fn rendered(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    // Frames a 60th of a second apart, so a click and another one inside the double-click time
    // are two frames, as by hand.
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("warned.pdf", None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        assert!(std::time::Instant::now() < deadline, "render readiness timed out: {:?}", h.state().views[0].page_errors());
        h.run_steps(2);
        if !h.state().render_pending() {
            h.run_steps(1);
            if !h.state().render_pending() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.state().views[0].page_errors().is_empty(), "{:?}", h.state().views[0].page_errors());
    h
}

#[test]
fn a_page_with_a_fallback_colour_is_marked_in_the_viewer() {
    // The tint transform pops more than it is given, so the colour can't be computed.
    let h = rendered(devicen_pdf("{ pop pop pop }"));
    assert_eq!(h.state().views[0].page_warnings(), vec![(0, &[RenderWarning::ColourFallback][..])]);
}

#[test]
fn a_page_drawn_as_specified_is_not_marked() {
    let h = rendered(devicen_pdf("{ pop pop 0 0 0 }"));
    assert!(h.state().views[0].page_warnings().is_empty(), "{:?}", h.state().views[0].page_warnings());
}

#[test]
fn the_warning_note_closes_with_its_button() {
    let mut h = rendered(devicen_pdf("{ pop pop pop }"));
    h.run_steps(2);
    assert!(h.state().views[0].render_warning_note(0).is_some());
    // The note's own close button: on the page, below any notice bar's Dismiss above the document.
    let close = h.get_all_by_label("Dismiss").max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).expect("the note's close button");
    close.click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].render_warning_note(0), None, "the note is closed");
    assert_eq!(h.state().views[0].page_warnings(), vec![(0, &[RenderWarning::ColourFallback][..])], "the page is still reported");
}

/// `devicen_pdf` with a second page drawn the same way, for full screen to have a page to turn to.
fn two_page_devicen_pdf(program: &str) -> Vec<u8> {
    let one = String::from_utf8(devicen_pdf(program)).expect("ASCII");
    let page = one.split("3 0 obj ").nth(1).and_then(|r| r.split(" endobj").next()).expect("page 1");
    one.replace("/Kids [3 0 R] /Count 1", "/Kids [3 0 R 6 0 R] /Count 2").replace("trailer", &format!("6 0 obj {page} endobj\ntrailer")).into_bytes()
}

/// CodeRabbit on #4501: in full screen a click turns the page unless it did something else;
/// closing the note is something else.
#[test]
fn closing_the_note_in_full_screen_does_not_turn_the_page() {
    let mut h = rendered(two_page_devicen_pdf("{ pop pop pop }"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while h.state().views[0].render_warning_note(0).is_none() || h.query_all_by_label("Dismiss").next().is_none() {
        assert!(std::time::Instant::now() < deadline, "the note in full screen");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let close = h.get_all_by_label("Dismiss").max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).expect("the note's close button");
    let at = close.rect().center();
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(30);
    assert_eq!(h.state().views[0].render_warning_note(0), None, "the note is closed");
    assert_eq!(h.state().views[0].current, 0, "and the page stays");
}

/// CodeRabbit on #4501: a click on the page queues a turn that waits out the double-click time; a
/// click on the note's close button in that time cancels it, as a followed link does.
#[test]
fn closing_the_note_cancels_a_page_turn_already_queued() {
    let mut h = rendered(two_page_devicen_pdf("{ pop pop pop }"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while h.state().views[0].render_warning_note(0).is_none() || h.query_all_by_label("Dismiss").next().is_none() {
        assert!(std::time::Instant::now() < deadline, "the note in full screen");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let close = h.get_all_by_label("Dismiss").max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).expect("the note's close button");
    let at = close.rect().center();
    // A click on the page, well below the note: it queues a turn.
    let page = egui::pos2(at.x - 200.0, at.y + 300.0);
    for pos in [page, at] {
        h.hover_at(pos);
        h.step();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.step();
    }
    h.run_steps(60);
    assert_eq!(h.state().views[0].render_warning_note(0), None, "the note is closed");
    assert_eq!(h.state().views[0].current, 0, "and the turn the page click queued is cancelled");
}
