//! The UI control channel (M3.9): an agent can see the widget tree, click, type, press keys,
//! run commands, change view options and take screenshots of the running app.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::control::{ControlClient, Reply};
use serde_json::{Value, json};

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_pages(5)
}

fn harness_pages(pages: usize) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let s = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *s.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("doc.pdf", None, fixture(pages)).unwrap();
        app
    });
    h.run_steps(4);
    let client = slot.lock().unwrap().take().unwrap();
    (h, client)
}

/// Send a request and run frames until it is answered.
fn call(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Reply {
    let rx = c.send(method, params);
    for _ in 0..30 {
        h.step();
        if let Ok(r) = rx.try_recv() {
            return r;
        }
    }
    panic!("{method}: no reply after 30 frames");
}

fn ok(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Value {
    call(h, c, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

#[test]
fn bookmark_titles_can_be_searched_over_control() {
    let (mut h, c) = harness();
    for (index, title, page) in [(0, "Background", 0), (1, "Target chapter", 3)] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark { parent: vec![], index, title: title.into(), page });
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "panel", "value": "bookmarks" }));
    h.run_steps(3);
    let widgets = ok(&mut h, &c, "ui.inspect", json!({ "query": "Search", "role": "TextInput" }));
    let rect = &widgets["widgets"][0]["rect"];
    let x = (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.0;
    let y = (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.0;
    ok(&mut h, &c, "ui.click", json!({ "x": x, "y": y }));
    ok(&mut h, &c, "ui.type", json!({ "text": "target" }));
    h.get_by_label("Target chapter");
    assert!(h.query_by_label("Background").is_none());
    ok(&mut h, &c, "ui.click", json!({ "label": "Target chapter" }));
    assert_eq!(h.state().views[0].current, 3);
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        h.render().unwrap().save(format!("{dir}/control-filtered-bookmarks.png")).unwrap();
    }
    ok(&mut h, &c, "ui.click", json!({ "label": "Clear" }));
    h.get_by_label("Background");
    h.get_by_label("Target chapter");
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        h.render().unwrap().save(format!("{dir}/control-cleared-bookmarks.png")).unwrap();
    }
}

#[test]
fn theme_commands_and_options_report_preference_and_effective_colours() {
    let (mut h, c) = harness();
    h.input_mut().system_theme = Some(egui::Theme::Dark);
    h.run_steps(2);
    for (id, preference, effective) in
        [("view.theme.system", "System", "Dark"), ("view.theme.light", "Light", "Light"), ("view.theme.dark", "Dark", "Dark")]
    {
        ok(&mut h, &c, "ui.command", json!({ "id": id }));
        let state = ok(&mut h, &c, "ui.state", json!({}));
        assert_eq!(state["theme_preference"], preference);
        assert_eq!(state["theme"], effective);
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "theme", "value": "system" }));
    h.input_mut().system_theme = Some(egui::Theme::Light);
    h.run_steps(2);
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["theme_preference"], "System");
    assert_eq!(state["theme"], "Light");
    assert!(call(&mut h, &c, "ui.set", json!({ "key": "theme", "value": "purple" })).is_err());
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["theme_preference"], "System");
}

/// Switching the interface language changes labels only: documents, their dirty state and the
/// command ids agents drive stay exactly the same.
#[test]
fn language_switch_preserves_document_and_command_ids() {
    use pdfcraft_ui_egui::i18n;
    let (mut h, c) = harness();
    let doc = h.state().views[0].id;
    h.state_mut().session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    assert_eq!(documents[0]["dirty"], true);
    let commands = ok(&mut h, &c, "ui.commands", json!({}));
    for code in ["ja", "zh-hans", "fr", "de", "en"] {
        ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": code }));
        h.run_steps(2);
        let state = ok(&mut h, &c, "ui.state", json!({}));
        assert_eq!(state["language"], code);
        assert_eq!(state["documents"], documents);
        assert_eq!(ok(&mut h, &c, "ui.commands", json!({})), commands);

        let lang = i18n::Lang::from_code(code).unwrap();
        h.get_by_label(i18n::tr(lang, "Menu")).click();
        h.run_steps(2);
        h.get_by_label(&format!("{} ⏵", i18n::tr(lang, "File"))).hover();
        h.run_steps(3);
        h.get_by_label_contains(i18n::tr(lang, "Open…"));
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        h.run_steps(2);
    }
    let error = call(&mut h, &c, "ui.set", json!({ "key": "language", "value": "xx" })).unwrap_err();
    assert!(error.contains("auto, en, ja"), "{error}");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "en");
}

#[cfg(target_os = "linux")]
fn start_autoscroll(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> egui::Pos2 {
    let p = h.state().views[0].viewport_rect().center();
    ok(h, c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(h.state().views[0].auto_scrolling());
    p
}

#[cfg(target_os = "linux")]
#[test]
fn middle_click_autoscroll_latches_has_a_dead_zone_and_scrolls_both_directions() {
    let (mut h, c) = harness();
    // Keep the tracked page visible through the downward leg and reversal.
    h.state_mut().set_option("zoom", "400").unwrap();
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(2);
    let p = start_autoscroll(&mut h, &c);
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    ok(&mut h, &c, "ui.move", json!({ "x": p.x + 100.0, "y": p.y + 8.0 }));
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(0).unwrap().top(), top, "horizontal motion and the dead zone do not scroll");
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 90.0 }));
    h.run_steps(8);
    let down = h.state().views[0].page_screen_rect(0).unwrap().top();
    assert!(down < top - 30.0, "moving below the anchor scrolls down: {top} -> {down}");
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y - 50.0 }));
    // The control request itself takes frames while the previous downward motion continues.
    // Measure reversal after it has arrived and the scroll area's layout has caught up.
    h.run_steps(2);
    let reversing = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    let up = h.state().views[0].page_screen_rect(0).unwrap().top();
    assert!(up > reversing + 15.0, "moving above the anchor scrolls up: {reversing} -> {up}");
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "a second wheel click stops");
    h.run_steps(2);
    let stopped = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(0).unwrap().top(), stopped, "no drift after cancellation");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "scrolling never edits the PDF");
}

#[cfg(target_os = "linux")]
#[test]
fn moving_before_middle_button_release_keeps_scrolling_without_starting_page_tools() {
    let (mut h, c) = harness();
    // Keep the tracked page on screen while the faster gesture continues after release.
    h.state_mut().set_option("zoom", "400").unwrap();
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(2);
    // A middle drag must not also draw with a selected tool (egui accepts any drag button).
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Crop;
    let p = h.state().views[0].viewport_rect().center();
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    ok(&mut h, &c, "ui.drag", json!({ "from": [p.x, p.y], "to": [p.x, p.y + 60.0], "steps": 12, "button": "middle" }));
    h.run_steps(2);
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0);
    assert!(h.state().views[0].auto_scrolling(), "release keeps scrolling toggled on even after movement");
    let released = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < released - 30.0, "scrolling continues with no button held");
    assert!(h.state().views[0].crop_drag.is_none(), "the Crop tool must not receive a wheel drag");
    assert!(h.state().dialog.is_none());
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y + 60.0, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "the next middle click toggles scrolling off");
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_uses_the_initial_click_position_when_input_arrives_in_one_frame() {
    let (mut h, c) = harness();
    let p = h.state().views[0].viewport_rect().center();
    let moved = p + egui::vec2(0.0, 60.0);
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.event(egui::Event::PointerMoved(p));
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Middle, pressed: true, modifiers: egui::Modifiers::NONE });
    h.event(egui::Event::PointerMoved(moved));
    h.event(egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(8);
    assert!(h.state().views[0].auto_scrolling());
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0, "distance is measured from the click, not the last mouse event");
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
}

#[cfg(target_os = "linux")]
#[test]
fn farther_from_the_click_scrolls_faster_in_the_viewer_and_page_grid() {
    use egui_kittest::kittest::Queryable;
    for organize in [false, true] {
        let (mut h, c) = harness_pages(40);
        h.state_mut().views[0].organize = organize;
        h.run_steps(3);
        let top = |h: &Harness<'static, PdfCraftApp>| {
            if organize { h.get_by_label("Page 1").rect().top() } else { h.state().views[0].page_screen_rect(0).unwrap().top() }
        };
        let p = start_autoscroll(&mut h, &c);
        ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 30.0 }));
        let before_near = top(&h);
        h.run_steps(8);
        let near = before_near - top(&h);
        ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 100.0 }));
        let before_far = top(&h);
        h.run_steps(8);
        let far = before_far - top(&h);
        assert!(near > 0.0 && far > near * 3.0, "farther movement must be faster: organize={organize}, near={near}, far={far}");
        ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y }));
        h.run_steps(2);
        let at_anchor = top(&h);
        h.run_steps(8);
        assert_eq!(top(&h), at_anchor, "returning to the original click pauses scrolling");
        assert!(h.state().views[0].auto_scrolling(), "the toggle stays on at the anchor");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_uses_elapsed_frame_time_and_preserves_fractional_motion_in_both_views() {
    use egui_kittest::kittest::Queryable;
    for (organize, scale, zoom) in [(false, 1.0, "100"), (false, 2.0, "100"), (false, 1.0, "400"), (true, 1.0, "100"), (true, 2.0, "100")] {
        for frames in [30, 60, 120, 144] {
            let (mut h, c) = harness_pages(40);
            h.set_pixels_per_point(scale);
            h.state_mut().set_option("zoom", zoom).unwrap();
            h.state_mut().views[0].organize = organize;
            h.run_steps(3);
            assert_eq!(h.ctx.pixels_per_point(), scale);
            let top = |h: &Harness<'static, PdfCraftApp>| {
                if organize { h.get_by_label("Page 1").rect().top() } else { h.state().views[0].page_screen_rect(0).unwrap().top() }
            };
            let step = |h: &mut Harness<'static, PdfCraftApp>| {
                // Deliberately differ from the harness's predicted frame interval: scrolling
                // must follow the elapsed time rather than the display's predicted rate.
                h.input_mut().time = Some(h.ctx.input(|i| i.time) + 1.0 / f64::from(frames));
                h.step();
            };
            let p = start_autoscroll(&mut h, &c);
            for (distance, expected) in [(16.0, 17.83), (50.0, 218.67)] {
                ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + distance }));
                step(&mut h);
                step(&mut h);
                let before = top(&h);
                for _ in 0..frames {
                    step(&mut h);
                }
                let travelled = before - top(&h);
                assert!(
                    (travelled - expected).abs() < 1.0,
                    "organize={organize}, scale={scale}, zoom={zoom}, fps={frames}, distance={distance}, travelled={travelled}"
                );
                assert_eq!(
                    h.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
                    std::time::Duration::ZERO,
                    "motion schedules the next display frame"
                );
            }
            ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 15.0 }));
            step(&mut h);
            let paused = top(&h);
            for _ in 0..frames {
                step(&mut h);
            }
            assert_eq!(top(&h), paused, "the dead zone pauses immediately, without coasting");
            assert!(h.state().views[0].auto_scrolling());
            ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y - 50.0 }));
            step(&mut h);
            step(&mut h);
            let before = top(&h);
            for _ in 0..frames {
                step(&mut h);
            }
            assert!((top(&h) - before - 218.67).abs() < 1.0, "resuming above the anchor reverses direction");
            ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn escape_stops_autoscroll_without_closing_find() {
    let (mut h, c) = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(3);
    // Focus the canvas so the text field no longer owns keyboard input.
    let p = h.state().views[0].viewport_rect().center();
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y }));
    h.run_steps(2);
    start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    assert!(!h.state().views[0].auto_scrolling());
    assert!(h.state().views[0].find.is_some(), "Escape cancels the scrolling gesture first");
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_cancels_on_click_wheel_focus_loss_and_pointer_exit() {
    let (mut h, c) = harness();
    let p = start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y }));
    assert!(!h.state().views[0].auto_scrolling());
    start_autoscroll(&mut h, &c);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -20.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    start_autoscroll(&mut h, &c);
    h.input_mut().focused = false;
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    h.input_mut().focused = true;
    h.run_steps(2);
    start_autoscroll(&mut h, &c);
    h.event(egui::Event::PointerGone);
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_is_scoped_to_the_active_view_and_cannot_start_under_a_dialog() {
    let (mut h, c) = harness();
    let p = start_autoscroll(&mut h, &c);
    h.state_mut().open_bytes("other.pdf", None, fixture(2)).unwrap();
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling(), "returning to the tab must not resume");
    start_autoscroll(&mut h, &c);
    h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::About);
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "a modal owns its input");
    assert!(call(&mut h, &c, "ui.move", json!({ "x": "bad", "y": 2 })).is_err());
    assert!(call(&mut h, &c, "ui.drag", json!({ "from": [1, 2], "to": [3, 4], "button": "bad" })).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn the_click_that_stops_autoscroll_preserves_the_page_selection() {
    use egui_kittest::kittest::Queryable;
    let (mut h, c) = harness_pages(40);
    h.state_mut().execute("page.organize");
    h.state_mut().views[0].select_pages(&[0]);
    h.run_steps(3);
    let page_two = h.get_by_label("Page 2").rect().center();
    start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.click", json!({ "x": page_two.x, "y": page_two.y }));
    assert!(!h.state().views[0].auto_scrolling());
    assert_eq!(h.state().views[0].target_pages(), vec![0], "the cancelling press and release belong to autoscroll");
    // The next ordinary click still selects normally.
    ok(&mut h, &c, "ui.click", json!({ "x": page_two.x, "y": page_two.y }));
    assert_eq!(h.state().views[0].target_pages(), vec![1]);
}

#[cfg(target_os = "linux")]
#[test]
fn organize_pages_supports_autoscroll_without_selecting_or_reordering_pages() {
    use egui_kittest::kittest::Queryable;
    let (mut h, c) = harness_pages(40);
    h.state_mut().execute("page.organize");
    h.run_steps(3);
    let top = h.get_by_label("Page 1").rect().top();
    let p = start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 80.0 }));
    h.run_steps(8);
    assert!(h.get_by_label("Page 1").rect().top() < top - 30.0, "the organize grid scrolls");
    assert!(h.state().views[0].selected.is_empty());
    assert!(h.state().views[0].org_drag.is_none());
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    // Holding the wheel over a page also must not create an organize drag.
    ok(&mut h, &c, "ui.drag", json!({ "from": [p.x, p.y], "to": [p.x, p.y + 80.0], "button": "middle" }));
    assert!(h.state().views[0].selected.is_empty());
    assert!(h.state().views[0].org_drag.is_none());
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    assert!(h.state().views[0].auto_scrolling(), "releasing the wheel leaves the grid scrolling on");
    h.state_mut().views[0].organize = false;
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling(), "changing canvas mode cancels the gesture");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn middle_button_input_does_not_start_custom_scrolling_outside_linux() {
    for organize in [false, true] {
        let (mut h, c) = harness_pages(40);
        h.state_mut().views[0].organize = organize;
        h.run_steps(3);
        let p = h.state().views[0].viewport_rect().center();
        ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
        ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 50.0 }));
        h.run_steps(8);
        assert!(!h.state().views[0].auto_scrolling(), "custom scrolling must be Linux-only: organize={organize}");
    }
}

#[test]
fn state_and_view_options() {
    let (mut h, c) = harness();
    let s = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(s["documents"][0]["name"], "doc.pdf");
    assert_eq!(s["documents"][0]["pages"], 5);
    assert_eq!(s["active"]["page"], 1);
    assert_eq!(s["active"]["page_errors"], json!([]));
    ok(&mut h, &c, "ui.set", json!({ "key": "page", "value": 4 }));
    h.run_steps(3);
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["active"]["page"], 4);
    ok(&mut h, &c, "ui.set", json!({ "key": "panel", "value": "bookmarks" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["right_panel"], "Bookmarks");
    assert!(call(&mut h, &c, "ui.set", json!({ "key": "panel", "value": "nonsense" })).is_err());
}

#[test]
fn japanese_controls_and_search_keep_command_ids() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "ja" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "ja");
    ok(&mut h, &c, "ui.click", json!({ "label": "閲覧" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Read");

    ok(&mut h, &c, "ui.command", json!({ "id": "app.preferences" }));
    let prefs = ok(&mut h, &c, "ui.inspect", json!({ "query": "表示言語" }));
    assert!(prefs["count"].as_u64().unwrap() > 0, "{prefs}");
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));

    for (query, translated) in [("整理", "ページを整理"), ("Split document", "文書を分割…"), ("page.split", "文書を分割…")] {
        ok(&mut h, &c, "ui.command", json!({ "id": "view.palette" }));
        ok(&mut h, &c, "ui.type", json!({ "text": query }));
        let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": translated }));
        assert!(hits["count"].as_u64().unwrap() > 0, "{query}: {hits}");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        h.state_mut().palette_query.clear();
    }
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "en" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "All tools" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "AllTools");
}

#[test]
fn preferences_menu_and_shortcut_allow_switching_interface_languages() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "ja" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "メニュー" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "編集 ⏵" }));
    let menu = ok(&mut h, &c, "ui.inspect", json!({ "query": "環境設定…" }));
    let prefs = menu["widgets"].as_array().unwrap().iter().find(|w| w["clickable"] == true).expect("Preferences menu item");
    ok(&mut h, &c, "ui.click", json!({ "id": prefs["id"] }));
    for (current, next, code) in [("日本語", "English", "en"), ("English", "日本語", "ja")] {
        let selector = ok(&mut h, &c, "ui.inspect", json!({ "query": current }));
        let combo = selector["widgets"].as_array().unwrap().iter().find(|w| w["role"] == "ComboBox").expect("language selector");
        ok(&mut h, &c, "ui.click", json!({ "id": combo["id"] }));
        ok(&mut h, &c, "ui.click", json!({ "label": next }));
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], code);
    }
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));
    ok(&mut h, &c, "ui.key", json!({ "key": ",", "modifiers": ["command"] }));
    let prefs = ok(&mut h, &c, "ui.inspect", json!({ "query": "表示言語" }));
    assert!(prefs["count"].as_u64().unwrap() > 0, "{prefs}");
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));

    ok(&mut h, &c, "ui.command", json!({ "id": "help.shortcuts" }));
    for label in ["キーボードショートカット", "開く", "環境設定", "次／前の検索結果", "ダブルクリック", "閉じる"]
    {
        let found = ok(&mut h, &c, "ui.inspect", json!({ "query": label }));
        assert!(found["count"].as_u64().unwrap() > 0, "{label}: {found}");
    }
    let english = ok(&mut h, &c, "ui.inspect", json!({ "query": "Next / previous match" }));
    assert_eq!(english["count"], 0);
    ok(&mut h, &c, "ui.click", json!({ "label": "閉じる" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");
}

#[test]
fn japanese_dialogs_errors_and_custom_action_names() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key": "language", "value": "ja"}));
    for (dialog, label) in [
        ("properties", "文書のプロパティ"),
        ("protect", "パスワードで保護"),
        ("export-image", "画像に書き出し"),
        ("optimize", "PDF の最適化"),
        ("recognize-text", "テキストを認識"),
        ("accessibility-options", "アクセシビリティチェックのオプション"),
        ("js-console", "JavaScript コンソール"),
        ("compare-files", "ファイルを比較"),
        ("sign", "署名用のデジタル ID を設定"),
    ] {
        ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": dialog}));
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["count"].as_u64().unwrap() > 0, "{dialog}: {found}");
    }
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "none"}));
    assert!(!h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0, 1, 2, 3, 4] }));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    // The frame is translated; the engine's own error text is shown as it is.
    assert_eq!(state["notice"], "操作「ページを削除」に失敗しました: a document must keep at least one page");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");

    h.state_mut().custom_actions.push(pdfcraft_engine::actions::Action {
        name: "Save".to_string(),
        description: "Print".to_string(),
        steps: Vec::new(),
        builtin: false,
    });
    h.state_mut().wizard.selected = Some("Save".to_string());
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "action-wizard"}));
    for label in ["アクションウィザード", "Save", "Print"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["widgets"].as_array().unwrap().iter().any(|w| w["label"] == label || w["value"] == label), "{found}");
    }
}

#[test]
fn simplified_chinese_about_tabs_and_credit_controls() {
    let (mut h, c) = harness();
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    ok(&mut h, &c, "ui.set", json!({"key": "language", "value": "zh-hans"}));
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "about"}));
    for label in ["关于", "贡献者", "模型"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["widgets"].as_array().unwrap().iter().any(|w| w["label"] == label), "{found}");
    }
    ok(&mut h, &c, "ui.click", json!({"label": "贡献者"}));
    for label in ["用户名", "显示名称", "真实姓名", "排序", "名称列表", "表格", "首次提交"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["count"].as_u64().unwrap() > 0, "{label}: {found}");
    }
    ok(&mut h, &c, "ui.click", json!({"label": "表格"}));
    for label in ["新增行", "删除行", "净增行", "新增资源", "删除资源"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["count"].as_u64().unwrap() > 0, "{label}: {found}");
    }
    ok(&mut h, &c, "ui.click", json!({"label": "模型"}));
    let columns = ok(&mut h, &c, "ui.inspect", json!({"query": "占全部提交的比例"}));
    let empty = ok(&mut h, &c, "ui.inspect", json!({"query": "此版本未包含模型贡献记录。"}));
    assert!(columns["count"].as_u64().unwrap() > 0 || empty["count"].as_u64().unwrap() > 0, "{columns}, {empty}");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"], documents);
}

#[test]
fn simplified_chinese_signature_prompts_and_errors_keep_document_state() {
    let (mut h, c) = harness();
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    ok(&mut h, &c, "ui.set", json!({"key": "language", "value": "zh-hans"}));
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "signature"}));
    let typed = ok(&mut h, &c, "ui.inspect", json!({"query": "请输入您的签名。"}));
    assert!(typed["count"].as_u64().unwrap() > 0, "{typed}");
    // The shell also has a Draw control; the modal's button is registered after the shell.
    let draw = ok(&mut h, &c, "ui.inspect", json!({"query": "绘制"}));
    let id = draw["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|w| w["label"] == "绘制" && w["clickable"] == true)
        .expect("signature Draw button")["id"]
        .clone();
    ok(&mut h, &c, "ui.click", json!({"id": id}));
    let drawn = ok(&mut h, &c, "ui.inspect", json!({"query": "请在下方绘制您的签名。"}));
    assert!(drawn["count"].as_u64().unwrap() > 0, "{drawn}");
    ok(&mut h, &c, "ui.click", json!({"label": "取消"}));
    assert!(!h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0, 1, 2, 3, 4] }));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["notice"], "删除页面失败：a document must keep at least one page");
    assert_eq!(state["documents"], documents, "a language change and failed edit must preserve the document");
}

#[test]
fn inspect_and_click_by_label_and_id() {
    let (mut h, c) = harness();
    let found = ok(&mut h, &c, "ui.inspect", json!({ "query": "read" }));
    let read = found["widgets"].as_array().unwrap().iter().find(|w| w["label"] == "Read" && w["clickable"] == true).cloned().expect("a Read tab");
    let rect = read["rect"].as_array().unwrap();
    assert!(rect[2].as_f64().unwrap() > rect[0].as_f64().unwrap());

    ok(&mut h, &c, "ui.click", json!({ "label": "Read" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Read");

    let all = ok(&mut h, &c, "ui.inspect", json!({ "query": "all tools" }));
    let id = all["widgets"].as_array().unwrap().iter().find(|w| w["label"] == "All tools" && w["clickable"] == true).unwrap()["id"].clone();
    ok(&mut h, &c, "ui.click", json!({ "id": id }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "AllTools");

    // Clicking a point works too (here: the Read tab's centre).
    let [x0, y0, x1, y1] = [0, 1, 2, 3].map(|i| rect[i].as_f64().unwrap());
    ok(&mut h, &c, "ui.click", json!({ "x": (x0 + x1) / 2.0, "y": (y0 + y1) / 2.0 }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Read");

    let err = call(&mut h, &c, "ui.click", json!({ "label": "No such button" })).unwrap_err();
    assert!(err.contains("no enabled clickable widget"), "{err}");
    assert!(call(&mut h, &c, "ui.click", json!({ "id": 12345 })).is_err());
}

#[test]
fn cover_page_command_needs_two_page_view() {
    // Agents see the cover toggle as disabled, and get an error, until two-page view.
    let (mut h, c) = harness();
    let enabled = |list: Value| list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "view.layout.cover").unwrap()["enabled"].clone();
    assert_eq!(enabled(ok(&mut h, &c, "ui.commands", json!({}))), false);
    let err = call(&mut h, &c, "ui.command", json!({ "id": "view.layout.cover" })).unwrap_err();
    assert!(err.contains("disabled"), "{err}");
    ok(&mut h, &c, "ui.command", json!({ "id": "view.layout.two_up" }));
    assert_eq!(enabled(ok(&mut h, &c, "ui.commands", json!({}))), true);
    ok(&mut h, &c, "ui.command", json!({ "id": "view.layout.cover" }));
    assert!(h.state().views[0].cover);
}

#[test]
fn commands_keys_and_typing() {
    let (mut h, c) = harness();
    let list = ok(&mut h, &c, "ui.commands", json!({}));
    let undo = list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "edit.undo").unwrap().clone();
    assert_eq!(undo["enabled"], false);
    assert!(call(&mut h, &c, "ui.command", json!({ "id": "edit.undo" })).unwrap_err().contains("disabled"));
    assert!(call(&mut h, &c, "ui.command", json!({ "id": "nope" })).is_err());

    ok(&mut h, &c, "ui.command", json!({ "id": "page.rotate" }));
    h.run_steps(2);
    let list = ok(&mut h, &c, "ui.commands", json!({}));
    assert_eq!(list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "edit.undo").unwrap()["enabled"], true);

    // ⌘K opens the palette; typing filters it; Escape closes it.
    ok(&mut h, &c, "ui.key", json!({ "key": "K", "modifiers": ["command"] }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["palette_open"], true);
    ok(&mut h, &c, "ui.type", json!({ "text": "split" }));
    let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": "split document" }));
    assert!(hits["count"].as_u64().unwrap() >= 1, "{hits}");
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["palette_open"], false);

    assert!(call(&mut h, &c, "ui.key", json!({ "key": "NotAKey" })).is_err());
    assert!(call(&mut h, &c, "ui.frobnicate", json!({})).unwrap_err().contains("unknown method"));
}

#[test]
fn select_all_key_selects_every_page_in_organize() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "organize", "value": "on" }));
    ok(&mut h, &c, "ui.set", json!({ "key": "select", "value": "3" }));
    ok(&mut h, &c, "ui.key", json!({ "key": "A", "modifiers": ["command"] }));
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2, 3, 4]);
    assert_eq!(h.state().views[0].current, 2);
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["dirty"], false);
}

#[test]
fn state_reports_the_selected_pages() {
    let (mut h, c) = harness();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["active"]["selected_pages"], json!([]));
    ok(&mut h, &c, "ui.set", json!({ "key": "select", "value": "2,4" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["active"]["selected_pages"], json!([2, 4]));
}

#[test]
fn saved_signature_can_be_changed_through_the_control_channel() {
    let (mut h, c) = harness();
    h.state_mut().signature = Some(pdfcraft_ui_egui::fill_sign::SavedSig::Typed("Ada Lovelace".into()));
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.signature.change" }));
    h.run_steps(3);
    assert_eq!(h.state().signature_draft.text, "Ada Lovelace");
    let fields = ok(&mut h, &c, "ui.inspect", json!({ "role": "TextInput" }));
    let field = fields["widgets"].as_array().unwrap().iter().find(|w| w["value"] == "Ada Lovelace").expect("the signature text field");
    let r = field["rect"].as_array().unwrap();
    let [x0, y0, x1, y1] = [0, 1, 2, 3].map(|i| r[i].as_f64().unwrap());
    ok(&mut h, &c, "ui.click", json!({ "x": (x0 + x1) / 2.0, "y": (y0 + y1) / 2.0 }));
    ok(&mut h, &c, "ui.key", json!({ "key": "A", "modifiers": ["command"] }));
    ok(&mut h, &c, "ui.type", json!({ "text": "Grace Hopper" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "Apply" }));
    assert_eq!(h.state().signature, Some(pdfcraft_ui_egui::fill_sign::SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["quick_tool"], "fill-signature");
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.signature.remove" }));
    assert_eq!(h.state().signature, None);
    h.state_mut().initials = Some(pdfcraft_ui_egui::fill_sign::SavedSig::Typed("GH".into()));
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.initials.remove" }));
    assert_eq!(h.state().initials, None);
}

#[test]
fn drawing_a_comment_by_drag_and_its_context_menu() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.command", json!({ "id": "comment.square" }));
    let st = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(st["quick_tool"], "square");
    let r = &st["active"]["pages_on_screen"][0]["rect"];
    let (x0, y0, x1, y1) = (r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap());
    let (a, b) = ([x0 + (x1 - x0) * 0.2, y0 + (y1 - y0) * 0.2], [x0 + (x1 - x0) * 0.5, y0 + (y1 - y0) * 0.4]);
    ok(&mut h, &c, "ui.drag", json!({ "from": a, "to": b }));
    h.run_steps(2);
    let st = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(st["active"]["selected_comment"], json!({ "page": 1, "index": 1 }), "{st}");
    assert_eq!(st["documents"][0]["dirty"], true);
    // A right-click on it offers the comment menu.
    ok(&mut h, &c, "ui.click", json!({ "x": (a[0] + b[0]) / 2.0, "y": (a[1] + b[1]) / 2.0, "button": "secondary" }));
    let menu = ok(&mut h, &c, "ui.inspect", json!({ "query": "Set status" }));
    assert!(menu["count"].as_u64().unwrap() >= 1, "{menu}");
    assert!(call(&mut h, &c, "ui.drag", json!({ "from": [1, 2] })).unwrap_err().contains("to must be"));
}

#[test]
fn screenshots_of_window_and_region() {
    let (mut h, c) = harness();
    let shot = ok(&mut h, &c, "ui.screenshot", json!({}));
    let ppp = shot["pixels_per_point"].as_f64().unwrap();
    assert_eq!(shot["width"].as_f64().unwrap(), (1400.0 * ppp).round());
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(shot["png_base64"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let region = ok(&mut h, &c, "ui.screenshot", json!({ "region": [10, 20, 110, 70] }));
    assert_eq!((region["width"].as_f64().unwrap(), region["height"].as_f64().unwrap()), ((100.0 * ppp).round(), (50.0 * ppp).round()));
    assert!(call(&mut h, &c, "ui.screenshot", json!({ "region": [5, 5, 1, 1] })).is_err());
}

#[test]
fn loopback_transport_requires_the_token() {
    use std::io::{BufRead, BufReader, Write};
    let (mut h, c) = harness();
    let ep = pdfcraft_ui_egui::control::serve(c).unwrap();
    let talk = |lines: Vec<Value>| {
        let port = ep.port;
        std::thread::spawn(move || {
            let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s).lines();
            let mut out = Vec::new();
            for l in lines {
                // The server closes the connection after a failed auth; later writes may fail.
                if writeln!(w, "{l}").is_err() {
                    break;
                }
                match r.next() {
                    Some(Ok(reply)) => out.push(serde_json::from_str::<Value>(&reply).unwrap()),
                    _ => break,
                }
            }
            out
        })
    };
    let pump = |h: &mut Harness<'static, PdfCraftApp>, t: std::thread::JoinHandle<Vec<Value>>| {
        while !t.is_finished() {
            h.step();
        }
        t.join().unwrap()
    };

    // Wrong token: rejected and disconnected before any request reaches the app.
    let bad = pump(
        &mut h,
        talk(vec![
            json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": "guess" } }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "ui.state" }),
        ]),
    );
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0]["error"]["code"], -32001);
    // No auth at all: same.
    let none = pump(&mut h, talk(vec![json!({ "jsonrpc": "2.0", "id": 1, "method": "ui.state" })]));
    assert_eq!(none[0]["error"]["code"], -32001);

    let good = pump(
        &mut h,
        talk(vec![
            json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "ui.state" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "ui.command", "params": { "id": "edit.undo" } }),
        ]),
    );
    assert_eq!(good[0]["result"]["ok"], true);
    assert_eq!(good[1]["result"]["documents"][0]["name"], "doc.pdf");
    assert!(good[2]["error"]["message"].as_str().unwrap().contains("disabled"));
}

#[test]
fn control_default_workspace_and_session_override() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key": "default-mode", "value": "edit"}));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["default_mode"], "edit");
    assert!(call(&mut h, &c, "ui.set", json!({"key": "default-mode", "value": "unknown"})).is_err());
    h.state_mut().open_bytes("next.pdf", None, fixture(1)).unwrap();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Edit");
    ok(&mut h, &c, "ui.set", json!({"key": "mode", "value": "read"}));
    h.state_mut().open_bytes("another.pdf", None, fixture(1)).unwrap();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Read");
}

#[test]
fn measurement_tools_draw_live_calibrate_save_and_export() {
    let (mut h, c) = harness_pages(1);
    let doc = h.state().views[0].id;
    h.state_mut().set_option("zoom", "100").unwrap();
    ok(&mut h, &c, "ui.command", json!({"id":"measure.scale"}));
    h.run_steps(3);
    h.state_mut().views[0].measure.drawing_points = 10.0;
    h.state_mut().views[0].measure.real_distance = 1.0;
    h.state_mut().views[0].measure.unit = "m".into();
    h.get_by_label("Apply scale").click();
    h.run_steps(3);
    let scale = h.state().session.get(doc).unwrap().measurement_scale(0, [20.0, 20.0]).unwrap();
    assert!((scale.x - 0.1).abs() < 1e-10);
    let click = |h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, x: f32, y: f32| {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        ok(h, c, "ui.click", json!({"x":r.left()+x*r.width()/200.0,"y":r.top()+y*r.height()/300.0}));
        h.run_steps(2);
    };
    for (command, points) in [
        ("measure.distance", vec![(20.0, 30.0), (80.0, 110.0)]),
        ("measure.perimeter", vec![(20.0, 130.0), (80.0, 130.0), (80.0, 210.0)]),
        ("measure.area", vec![(100.0, 130.0), (160.0, 130.0), (160.0, 210.0), (100.0, 210.0)]),
    ] {
        ok(&mut h, &c, "ui.command", json!({"id":command}));
        for (x, y) in points {
            click(&mut h, &c, x, y);
        }
        if command != "measure.distance" {
            ok(&mut h, &c, "ui.key", json!({"key":"Enter"}));
            h.run_steps(3);
        }
    }
    let measurements = h.state().session.get(doc).unwrap().measurements().unwrap().measurements;
    assert_eq!(measurements.len(), 3);
    for (m, value) in measurements.iter().zip([10.0, 14.0, 48.0]) {
        assert!((m.reading.value - value).abs() < 0.01, "{m:?}");
    }
    ok(&mut h, &c, "ui.command", json!({"id":"edit.undo"}));
    h.run_steps(2);
    assert_eq!(h.state().session.get(doc).unwrap().measurements().unwrap().measurements.len(), 2);
    ok(&mut h, &c, "ui.command", json!({"id":"edit.redo"}));
    h.run_steps(2);
    let dir = std::env::temp_dir().join(format!("pdfcraft-measure-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into());
    ok(&mut h, &c, "ui.command", json!({"id":"measure.export"}));
    h.run_steps(2);
    assert!(std::fs::read_to_string(dir.join("measurements.csv")).unwrap().contains("m^2"));
    h.state_mut().set_option("quick", "measure-calibrate").unwrap();
    click(&mut h, &c, 30.0, 50.0);
    click(&mut h, &c, 130.0, 50.0);
    assert!((h.state().views[0].measure.drawing_points - 100.0).abs() < 0.01);
    let _ = std::fs::remove_dir_all(dir);
}
