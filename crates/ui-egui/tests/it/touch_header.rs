//! Responsive headers remain reachable with touch, including release followed by PointerGone.

use egui::{Event, Pos2, Rect, TouchPhase, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{Mode, PdfCraftApp, i18n};

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

fn header(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<Rect> {
    // The menu bar shares the row and has an Edit menu of its own: the mode tab is the taller.
    h.query_all_by_label(label)
        .map(|node| node.rect())
        .filter(|rect| (38.0..88.0).contains(&rect.center().y))
        .max_by(|a, b| a.height().total_cmp(&b.height()))
}

fn reachable(h: &Harness<'static, PdfCraftApp>, label: &str, rect: Rect) {
    assert!(rect.width() >= 44.0 && rect.height() >= 44.0, "{label}: {rect:?}");
    assert!(h.ctx.content_rect().contains_rect(rect), "{label} outside viewport: {rect:?}");
}

fn save_shortcut(h: &Harness<'static, PdfCraftApp>) -> String {
    if h.ctx.os() == egui::os::OperatingSystem::Mac { "⌘S".to_owned() } else { format!("{}+S", i18n::key_name("Ctrl")) }
}

fn check_header(h: &Harness<'static, PdfCraftApp>, language: i18n::Lang, width: f32) {
    let mut targets = Vec::new();
    for key in ["Find tools and commands", "Save ({key})"] {
        let label = i18n::tr(language, key).replace("{key}", &save_shortcut(h));
        let rect = header(h, &label).unwrap_or_else(|| panic!("missing {label} at {width}"));
        reachable(h, &label, rect);
        targets.push((label.to_owned(), rect));
    }
    for key in ["All tools", "Edit", "Convert", "E-Sign"] {
        let label = i18n::tr(language, key).replace("{key}", &save_shortcut(h));
        if let Some(rect) = header(h, &label) {
            reachable(h, &label, rect);
            targets.push((label.to_owned(), rect));
        }
    }
    for key in ["Print ({key})", "Document properties ({key})"] {
        let label = i18n::tr(language, key).split("{key}").next().unwrap();
        let target = h.query_all_by_label_contains(label).map(|node| node.rect()).find(|rect| (38.0..88.0).contains(&rect.center().y));
        if width >= 600.0 {
            let rect = target.unwrap();
            reachable(h, label, rect);
            targets.push((label.to_owned(), rect));
        } else {
            assert!(target.is_none(), "compact header moves {label} into the menu");
        }
    }
    for (i, (name, rect)) in targets.iter().enumerate() {
        for (other, other_rect) in targets.iter().skip(i + 1) {
            assert!(!rect.intersects(*other_rect), "{width}: {name} {rect:?} overlaps {other} {other_rect:?}");
        }
    }
}

fn screenshot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    // Optional visual verification uses one GPU-backed harness per process.
    if std::env::var("PDFCRAFT_TOUCH_SHOT_NAME").ok().as_deref() == Some(name)
        && let Some(directory) = std::env::var_os("PDFCRAFT_TOUCH_SHOTS")
    {
        // Only one wgpu device at a time in this test binary.
        let _gpu = crate::gpu();
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(directory.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn touch_headers_fit_and_modes_and_search_work_in_english_and_german() {
    for language in crate::support::english_and("de") {
        let lang = i18n::Lang::from_code(language).unwrap();
        for width in [390.0, 768.0, 1024.0] {
            let mut h = harness(language, width);
            check_header(&h, lang, width);
            // The mode bar has no Read tab: reading is the viewer with no tool in use.
            let dropdown = header(&h, i18n::tr(lang, "Convert")).is_none();
            assert!(width != 390.0 || dropdown, "phone width uses a mode dropdown");
            assert!(width != 1024.0 || !dropdown, "landscape tablet has room for the mode tabs");
            let mut current = "All tools";
            for (mode, key) in [(Mode::Edit, "Edit"), (Mode::Convert, "Convert"), (Mode::Sign, "E-Sign"), (Mode::AllTools, "All tools")] {
                if dropdown {
                    let trigger = header(&h, i18n::tr(lang, current)).unwrap();
                    tap(&mut h, trigger.center());
                    let item = h.get_by_label(i18n::tr(lang, key)).rect();
                    reachable(&h, key, item);
                    tap(&mut h, item.center());
                } else {
                    let tab = header(&h, i18n::tr(lang, key)).unwrap();
                    tap(&mut h, tab.center());
                }
                assert_eq!(h.state().mode, mode, "{language}/{width}: touch selects {key}");
                current = key;
                check_header(&h, lang, width);
            }
            h.state_mut().set_option("left", "closed").unwrap();
            h.run_steps(2);
            screenshot(&mut h, &format!("header-{language}-{width}"));
            let search = header(&h, i18n::tr(lang, "Find tools and commands")).unwrap();
            tap(&mut h, search.center());
            assert!(h.state().palette_open, "{language}/{width}: touch opens command search");
            let id = h.state().views[0].id;
            assert!(!h.state().session.get(id).unwrap().dirty, "header navigation never marks the PDF");
        }
    }
}

#[test]
fn touch_save_activates_the_existing_save_path_at_each_width() {
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    for language in crate::support::english_and("de") {
        for width in [390.0, 768.0, 1024.0] {
            let path = std::env::temp_dir().join(format!("pdfcraft-touch-header-{}-{unique}-{language}-{width}.pdf", std::process::id()));
            let mut h = harness(language, width);
            h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
            h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
            h.run_steps(2);
            let id = h.state().views[0].id;
            assert!(h.state().session.get(id).unwrap().dirty);
            let label = i18n::tr(i18n::Lang::from_code(language).unwrap(), "Save ({key})").replace("{key}", &save_shortcut(&h));
            let save = header(&h, &label).unwrap();
            reachable(&h, &label, save);
            tap(&mut h, save.center());
            assert!(!h.state().session.get(id).unwrap().dirty);
            let bytes = std::fs::read(&path).unwrap();
            PdfCraftApp::new().open_bytes("saved.pdf", None, bytes).unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }
}
