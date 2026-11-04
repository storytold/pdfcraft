//! Phone tab strips and horizontally scrolling tools stay usable without a hover cursor.

use egui::{Event, Pos2, Rect, TouchPhase, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, i18n};

fn page_bytes() -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>"]
            .iter()
            .enumerate()
    {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn harness(language: &'static str, width: f32) -> Harness<'static, PdfCraftApp> {
    let height = if width == 768.0 { 1024.0 } else { 768.0 };
    let mut h = Harness::builder().with_size(vec2(width, height)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", language).unwrap();
        app.set_option("touch-mode", "on").unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.open_bytes("tablet.pdf", None, page_bytes()).unwrap();
        app
    });
    h.run_steps(5);
    h
}

fn tap(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    let contact = |phase| Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None };
    let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(pos), contact(TouchPhase::Start), button(true)]);
    h.run_steps(1);
    h.input_mut().events.extend([button(false), contact(TouchPhase::End), Event::PointerGone]);
    h.run_steps(3);
}

fn swipe(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    let contact = |phase, pos| Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None };
    let button = |pressed, pos| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(from), contact(TouchPhase::Start, from), button(true, from)]);
    h.run_steps(1);
    for step in 1..=12 {
        let pos = from.lerp(to, step as f32 / 12.0);
        h.input_mut().events.extend([Event::PointerMoved(pos), contact(TouchPhase::Move, pos)]);
        h.run_steps(1);
    }
    // Pause before releasing, avoiding momentum in this deterministic reachability check.
    h.run_steps(12);
    h.input_mut().events.extend([button(false, to), contact(TouchPhase::End, to), Event::PointerGone]);
    h.run_steps(4);
}

fn screenshot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    // Optional private previews use one GPU-backed harness per process, never in normal tests.
    if std::env::var("PDFCRAFT_TOUCH_SHOT_NAME").ok().as_deref() == Some(name)
        && let Some(directory) = std::env::var_os("PDFCRAFT_TOUCH_SHOTS")
    {
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            h.run_steps(1);
        }
        // Only one wgpu device at a time in this test binary.
        let _gpu = crate::gpu();
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(directory.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn tab_controls_do_not_overlap_at_phone_and_tablet_widths() {
    for language in crate::support::english_and("de") {
        let lang = i18n::Lang::from_code(language).unwrap();
        for width in [390.0, 768.0, 1024.0] {
            let mut h = harness(language, width);
            for count in [1, 12] {
                for i in h.state().views.len()..count {
                    h.state_mut().open_bytes(&format!("document-{i}.pdf"), None, page_bytes()).unwrap();
                }
                h.run_steps(4);
                let mut controls = vec![
                    ("Open", h.get_by_label(i18n::tr(lang, "Open")).rect()),
                    ("Home", h.get_by_label(i18n::tr(lang, "Home")).rect()),
                    ("Discord", h.get_by_label("Discord").rect()),
                    ("Keyboard shortcuts", h.get_by_label(i18n::tr(lang, "Keyboard shortcuts")).rect()),
                ];
                controls.push(("Theme", h.get_by_label_contains(i18n::tr(lang, "Display theme")).rect()));
                for (i, (name, rect)) in controls.iter().enumerate() {
                    assert!(h.ctx.content_rect().contains_rect(*rect), "{language}/{width}: {name} outside viewport: {rect:?}");
                    for (other, other_rect) in controls.iter().skip(i + 1) {
                        assert!(!rect.intersects(*other_rect), "{language}/{width}: {name} {rect:?} overlaps {other} {other_rect:?}");
                    }
                }
                if count == 1 {
                    screenshot(&mut h, &format!("phone-header-{language}-{width}"));
                }
            }
        }
    }
}

#[test]
fn finger_swipes_reach_undo_and_redo_without_selecting_tools_or_drawing() {
    for language in crate::support::english_and("de") {
        let lang = i18n::Lang::from_code(language).unwrap();
        for width in [390.0, 768.0, 1024.0] {
            let mut h = harness(language, width);
            h.state_mut().set_option("quick", "ink").unwrap();
            h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
            h.run_steps(4);
            let tool = h.state().quick_tool;
            let y = h.get_by_label(i18n::tr(lang, "Draw")).rect().center().y;
            let visible = Rect::from_min_max(egui::pos2(0.0, y - 22.0), egui::pos2(width - 48.0, y + 22.0));
            if width == 390.0 {
                assert!(!visible.contains_rect(h.get_by_label(i18n::tr(lang, "Undo")).rect()));
                swipe(&mut h, egui::pos2(width - 80.0, y), egui::pos2(60.0, y));
            }
            assert_eq!(h.state().quick_tool, tool, "a scrolling gesture must not select the tool it started on");
            let id = h.state().views[0].id;
            for (key, expected) in [("Undo", 0), ("Redo", 90)] {
                let rect = h.get_by_label(i18n::tr(lang, key)).rect();
                assert!(visible.contains_rect(rect), "{language}/{width}: {key} {rect:?} outside toolbar {visible:?}");
                assert!(rect.width() >= 44.0 && rect.height() >= 44.0);
                tap(&mut h, rect.center());
                assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, expected, "touch activates {key}");
            }
            assert!(h.state().session.get(id).unwrap().info.annotations.is_empty(), "toolbar drags never annotate the page");
            screenshot(&mut h, &format!("phone-tools-{language}-{width}"));
            if width == 390.0 {
                swipe(&mut h, egui::pos2(60.0, y), egui::pos2(width - 80.0, y));
                let hand = h.get_by_label(i18n::tr(lang, "Hand (H)")).rect();
                assert!(visible.contains_rect(hand), "swipe back reaches the first tool");
                tap(&mut h, hand.center());
                assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::Hand);
            }
        }
    }
}
