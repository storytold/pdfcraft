//! The Print dialog in the real shell (egui_kittest): settings, preview sheets, Save as PDF.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with_settings(None)
}

fn harness_with_settings(settings: Option<String>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if let Some(json) = &settings {
            app.restore(json);
        }
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn current_print_page_saves_the_viewed_page_when_labels_restart() {
    // Five distinct painted pages, labelled i, ii, iii, 1, 2.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /r >> 3 << /S /D /St 1 >>] >> >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R 9 0 R 11 0 R] /Count 5 /MediaBox [0 0 200 300] >>".to_owned(),
    ];
    for (i, color) in ["1 0 0", "0 1 0", "0 0 1", "1 1 0", "1 0 1"].iter().enumerate() {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Resources << >> /Contents {} 0 R >>", 4 + i * 2));
        let content = format!("{color} rg 0 0 200 300 re f");
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("labelled.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    let labels: Vec<_> = h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["i", "ii", "iii", "1", "2"]);
    for (current, expected) in [[255, 0, 0], [0, 255, 0]].into_iter().enumerate() {
        h.state_mut().set_option("page", &(current + 1).to_string()).unwrap();
        h.run_steps(3);
        assert!(h.state_mut().execute("print.dialog"));
        h.run_steps(3);
        h.get_by_label("Current page").click();
        h.run_steps(3);
        assert_eq!(h.state().print_draft.current_page, current);
        h.get_by_label("Sheet 1 of 1");
        let out = std::env::temp_dir().join(format!("pdfcraft-print-current-{}-{current}.pdf", std::process::id()));
        h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
        h.state_mut().print_draft.printer = None;
        h.run_steps(1);
        h.get_by_label("Save as PDF").click();
        h.run_steps(3);
        assert_eq!(h.state().dialog, None);
        let saved = std::sync::Arc::new(std::fs::read(&out).unwrap());
        std::fs::remove_file(out).unwrap();
        let doc = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
        let mut renderer = pdfcraft_render::PageRenderer::new(saved, Default::default());
        let shot = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(shot.error.is_none(), "{:?}", shot.error);
        let center = ((shot.height / 2 * shot.width + shot.width / 2) * 4) as usize;
        assert_eq!(&shot.rgba[center..center + 3], &expected, "saved the wrong physical page for {current}");
    }
}

#[test]
fn print_dialog_lays_out_sheets_and_saves_a_pdf() {
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Print));
    h.get_by_label("Pages to Print".to_uppercase().as_str());
    h.get_by_label("Sheet 1 of 1");
    // Two copies of the one page, as a 2-up poster-free layout: Multiple.
    h.get_by_label("Multiple").click();
    h.run_steps(2);
    h.get_by_label("Sheet 1 of 1");
    // No printer on a test machine: Save as PDF.
    let out = std::env::temp_dir().join(format!("pdfcraft-print-test-{}.pdf", std::process::id()));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().print_draft.printer = None;
    h.run_steps(1);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let bytes = std::fs::read(&out).expect("saved");
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
    let _ = std::fs::remove_file(out);
}

/// Pages picked beforehand are what the dialog offers to print (Acrobat's "Selected pages").
#[test]
fn print_dialog_prints_the_selected_pages() {
    use pdfcraft_ui_egui::PrintWhich;
    let mut h = harness();
    let bytes = h.state().session.create_blank(200.0, 300.0, 5).unwrap();
    h.state_mut().open_bytes("five.pdf", None, bytes.as_ref().clone()).unwrap();
    h.run_steps(3);
    let tab = h.state().views.len() - 1;
    // Nothing picked: the whole document, and no "Selected pages" choice.
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    assert_eq!(h.state().print_draft.which, PrintWhich::All);
    assert!(h.query_by_label_contains("Selected pages").is_none());
    h.get_by_label("Sheet 1 of 5");
    h.state_mut().dialog = None;
    h.run_steps(2);
    // Pages 2 and 4 picked.
    h.state_mut().views[tab].select_pages(&[1, 3]);
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    assert_eq!(h.state().print_draft.which, PrintWhich::Selected);
    h.get_by_label("Selected pages (2)");
    h.get_by_label("Sheet 1 of 2");
    let out = std::env::temp_dir().join(format!("pdfcraft-print-selected-{}.pdf", std::process::id()));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().print_draft.printer = None;
    h.run_steps(1);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let saved = std::fs::read(&out).expect("saved");
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(saved)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 2);
    let _ = std::fs::remove_file(out);
    // The selection gone, the remembered choice falls back to the whole document.
    h.state_mut().views[tab].select_pages(&[]);
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    assert_eq!(h.state().print_draft.which, PrintWhich::All);
    assert!(h.state().print_draft.selected.is_empty());
}

#[test]
fn invalid_ranges_are_explained_in_the_preview() {
    let mut h = harness();
    h.state_mut().execute("print.dialog");
    h.run_steps(2);
    h.state_mut().print_draft.which = pdfcraft_ui_egui::PrintWhich::Range;
    h.state_mut().print_draft.range = "7".into();
    h.run_steps(2);
    h.get_by_label_contains("out of range");
}

#[test]
fn cut_stack_dialog_previews_saves_and_refuses_duplex() {
    use pdfcraft_engine::print::{PageOrder, spool::Duplex};
    let mut h = harness();
    // Ten source pages, generated using the headless engine.
    let bytes = h.state().session.create_blank(200.0, 300.0, 10).unwrap();
    h.state_mut().open_bytes("numbered.pdf", None, bytes.as_ref().clone()).unwrap();
    h.state_mut().execute("print.dialog");
    h.run_steps(3);
    h.get_by_label("Multiple").click();
    h.run_steps(2);
    // Pick the new order through the actual widgets.
    h.get_by_value("Horizontal").click();
    h.run_steps(2);
    h.get_by_label("Cut and stack").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.order, PageOrder::CutStack);
    h.state_mut().print_draft.per_sheet = 4;
    h.run_steps(2);
    h.get_by_label("Sheet 1 of 3");
    h.get_by_label_contains("Keep the sheets in order");
    h.get_by_label("›").click();
    h.run_steps(2);
    h.get_by_label("Sheet 2 of 3");
    h.state_mut().print_draft.duplex = Some(Duplex::LongEdge);
    h.run_steps(2);
    h.get_by_label_contains("Cut and stack needs Two-sided: Off");
    assert!(!h.state_mut().print_now());
    h.state_mut().print_draft.duplex = Some(Duplex::Off);
    h.state_mut().print_draft.printer = None;
    let out = std::env::temp_dir().join(format!("pdfcraft-cut-stack-ui-{}.pdf", std::process::id()));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.run_steps(2);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let printed = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(&out).unwrap())).unwrap();
    assert_eq!(pdfcraft_model::pages(&printed).len(), 3);
    let _ = std::fs::remove_file(out);
}

#[test]
fn a_failed_save_as_pdf_keeps_the_dialog_open() {
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.state_mut().print_draft.printer = None;
    // A folder that doesn't exist: the write fails.
    let out = std::env::temp_dir().join(format!("pdfcraft-missing-{}", std::process::id())).join("x.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.run_steps(2);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Print), "the dialog stays open, with its settings");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("Could not save"), "the user is told why: {toast:?}");
}

/// Properties… shows the printer driver's own options (CUPS) and sends only those changed.
#[test]
fn print_dialog_shows_the_printer_driver_options() {
    use pdfcraft_engine::print::spool;
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.state_mut().print_draft.printer = None;
    h.run_steps(1);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Properties…") && n.is_disabled()).is_some(), "Save as PDF has no driver");
    let tray = spool::PrinterOption {
        key: "InputSlot".into(),
        label: "Paper tray".into(),
        group: "Media".into(),
        choices: vec![("AutoSelect".into(), "Auto tray select".into()), ("Tray2".into(), "Tray 2".into())],
        default: "AutoSelect".into(),
    };
    {
        let d = &mut h.state_mut().print_draft;
        d.printer = Some("Test_Queue".into());
        d.driver_options = Some(("Test_Queue".into(), vec![tray]));
        d.driver_choices.insert("InputSlot".into(), "Tray2".into());
        d.show_driver_options = true;
    }
    h.run_steps(2);
    h.get_by_label("Printer properties");
    h.get_by_label("Media");
    h.get_by_label("Paper tray");
    // The selected sheet's media is always sent, independently of changed driver properties.
    let media = vec![("media".to_string(), "Letter".to_string())];
    assert_eq!(h.state().print_draft.job("form.pdf").options, [("InputSlot".to_string(), "Tray2".to_string()), media[0].clone()]);
    h.get_by_label("Reset to printer defaults").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.job("form.pdf").options, media, "driver defaults are not sent; selected paper is retained");
    // Options read for another printer never go with this one's job.
    h.state_mut().print_draft.driver_choices.insert("InputSlot".into(), "Tray2".into());
    h.state_mut().print_draft.printer = Some("Other_Queue".into());
    assert_eq!(h.state().print_draft.job("form.pdf").options, media, "another queue must not inherit driver options");
}

/// The desktop app lists printers in the background (PowerShell on Windows takes a second or
/// more): the dialog opens at once, then preselects the default when the list arrives — unless
/// the user has already picked a printer.
#[test]
fn printers_listed_in_the_background_preselect_the_default_unless_one_was_picked() {
    use pdfcraft_engine::print::spool::Printer;
    use std::sync::{Arc, Mutex};
    let mut h = harness();
    // The source waits until the test lets it answer, so the dialog is seen while it looks.
    let gate = Arc::new(Mutex::new(()));
    let hold = gate.lock().unwrap();
    let wait = gate.clone();
    h.state_mut().printer_source = Some(Arc::new(move || {
        let _open = wait.lock();
        vec![
            Printer { name: "Office".into(), default: false, two_sided: None },
            Printer { name: "Front Desk".into(), default: true, two_sided: None },
        ]
    }));
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Print), "the dialog doesn't wait for the printers");
    assert!(h.state().print_draft.printers_loading);
    assert_eq!(h.state().print_draft.printer, None);
    // Nothing is known yet (#160): the dialog says it is looking, and Print waits instead of
    // saving a PDF.
    h.get_by_value("Looking for printers…");
    assert!(h.get_by_role_and_label(Role::Button, "Print").accesskit_node().is_disabled(), "Print waits for the list");
    assert!(h.query_by_label("Save as PDF").is_none(), "not offered as the choice while looking");
    drop(hold);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().print_draft.printers_loading && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().print_draft.printers_loading);
    assert_eq!(h.state().print_draft.printers.len(), 2);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Front Desk"));

    // Opened again: last time's list shows at once; a choice made before the fresh list
    // arrives is kept.
    h.state_mut().dialog = None;
    let hold = gate.lock().unwrap();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    assert_eq!(h.state().print_draft.printers.len(), 2, "the last list shows while looking");
    {
        let d = &mut h.state_mut().print_draft;
        d.printer = None;
        d.printer_picked = true;
    }
    drop(hold);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().print_draft.printers_loading && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().print_draft.printers_loading);
    assert_eq!(h.state().print_draft.printer, None, "Save as PDF, as picked");
}

/// The printer picked in the dialog, Save as PDF included, is still chosen the next time Print
/// opens (#3537), until a fresh list no longer has it.
#[test]
fn the_picked_printer_is_kept_for_the_next_print_while_it_is_listed() {
    use pdfcraft_engine::print::spool::Printer;
    use std::sync::{Arc, Mutex};
    let mut h = harness();
    let listed = Arc::new(Mutex::new(vec!["Office", "Front Desk"]));
    let source = listed.clone();
    h.state_mut().printer_source = Some(Arc::new(move || {
        let names = source.lock().map(|n| n.clone()).unwrap_or_default();
        names.into_iter().map(|name| Printer { name: name.into(), default: name == "Front Desk", two_sided: None }).collect()
    }));
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().dialog = None;
        assert!(h.state_mut().execute("print.dialog"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.state().print_draft.printers_loading && std::time::Instant::now() < deadline {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!h.state().print_draft.printers_loading);
        h.run_steps(2);
    };
    open(&mut h);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Front Desk"), "the default first");

    // As the Printer list records a choice.
    {
        let d = &mut h.state_mut().print_draft;
        d.printer = None;
        d.printer_picked = true;
    }
    open(&mut h);
    assert_eq!(h.state().print_draft.printer, None, "Save as PDF, as picked last time");

    {
        let d = &mut h.state_mut().print_draft;
        d.printer = Some("Office".into());
        d.printer_picked = true;
    }
    open(&mut h);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Office"), "Office, as picked last time");

    // Unplugged: the fresh list doesn't have it, so the dialog goes back to the default.
    *listed.lock().unwrap() = vec!["Front Desk"];
    open(&mut h);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Front Desk"));
    assert!(!h.state().print_draft.printer_picked);
}

#[test]
fn architectural_page_can_be_matched_at_100_percent() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(2592.0, 1728.0, 1).unwrap();
        app.open_bytes("architectural.pdf", None, bytes.as_ref().clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.get_by_label("PDF page: 36.00 × 24.00 in");
    h.get_by_label("Match page at 100%").click();
    h.run_steps(3);
    h.get_by_value("ARCH D (24 x 36 in)");
    h.get_by_label("PDF scale: 100.00%");
    h.get_by_label("36.00 × 24.00 in");
    h.get_by_label("Printer paper and scaling may differ. Check driver settings.");
    let settings = h.state().print_draft.settings(1, &[]).unwrap();
    assert_eq!(settings.paper, (1728.0, 2592.0));
    assert_eq!(settings.layout, pdfcraft_engine::print::Layout::Size(pdfcraft_engine::print::SizeMode::Actual));
}

#[test]
fn custom_page_cannot_silently_match_a_standard_paper() {
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    // The 300 x 400 pt fixture is not a standard sheet.
    assert!(pdfcraft_engine::print::matching_paper((300.0, 400.0)).is_none());
    let before = h.state().print_draft.clone();
    assert!(h.get_by_label("Match page at 100%").accesskit_node().is_disabled());
    h.get_by_label("Match page at 100%").click();
    h.run_steps(3);
    assert_eq!(h.state().print_draft.paper, before.paper);
    assert_eq!(h.state().print_draft.size, before.size);
}

/// A driver Windows reports without two-sided printing keeps its own setting whatever Two-sided
/// says (some print both sides with it Off), so the dialog says where to change it.
#[test]
fn a_printer_without_two_sided_printing_points_to_its_properties() {
    use pdfcraft_engine::print::spool::Printer;
    let mut h = harness();
    h.state_mut().printer_source = Some(std::sync::Arc::new(|| {
        vec![
            Printer { name: "Office PCL".into(), default: true, two_sided: Some(true) },
            Printer { name: "Office PS".into(), default: false, two_sided: Some(false) },
        ]
    }));
    assert!(h.state_mut().execute("print.dialog"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().print_draft.printers.is_empty() && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
    let note = "doesn't take two-sided settings";
    assert!(h.query_by_label_contains(note).is_none(), "a driver that prints two-sided as asked");
    h.state_mut().print_draft.printer = Some("Office PS".into());
    h.run_steps(2);
    h.get_by_label_contains(note);
}

/// Print as image (Acrobat's Advanced Print Setup) is a choice in the Print dialog's Advanced
/// section: the saved print-ready PDF carries the page as a picture, no text.
#[test]
fn print_as_image_is_an_advanced_choice_that_saves_pictures() {
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.get_by_label("Advanced".to_uppercase().as_str()).click();
    h.run_steps(2);
    h.get_by_label("Print as image").click();
    h.run_steps(2);
    assert!(h.state().print_draft.as_image);
    let out = std::env::temp_dir().join(format!("pdfcraft-print-image-test-{}.pdf", std::process::id()));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().print_draft.printer = None;
    h.run_steps(1);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let bytes = std::fs::read(&out).expect("saved");
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
    let pictures =
        doc.scan_objects().filter(|(_, o)| matches!(&**o, pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image"))).count();
    assert_eq!(pictures, 1, "the page is one picture");
    let _ = std::fs::remove_file(out);
}

/// The spooler answering with no printers at all says so (#160), and Save as PDF still works.
#[test]
fn an_empty_printer_list_says_none_were_found() {
    use std::sync::Arc;
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(Vec::new));
    assert!(h.state_mut().execute("print.dialog"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().print_draft.printers_loading && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label_contains("No printers were found");
    assert!(!h.get_by_role_and_label(Role::Button, "Save as PDF").accesskit_node().is_disabled());
}

/// `PAPERS` index of the named sheet.
fn paper(name: &str) -> usize {
    pdfcraft_engine::print::PAPERS.iter().position(|p| p.0 == name).unwrap()
}

/// Label loads label stock; leaving it gives the paper the person had back (#3564).
#[test]
fn leaving_label_gives_back_the_paper_it_replaced() {
    use pdfcraft_engine::print::LABEL_PAPER;
    use pdfcraft_ui_egui::PrintHandling;
    let a4 = paper("A4");
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    for (mode, tab) in
        [(PrintHandling::Size, "Size"), (PrintHandling::Poster, "Poster"), (PrintHandling::Multiple, "Multiple"), (PrintHandling::Booklet, "Booklet")]
    {
        h.state_mut().print_draft.paper = a4;
        h.state_mut().print_draft.handling = PrintHandling::Size;
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Label").click();
        h.run_steps(2);
        assert_eq!(h.state().print_draft.handling, PrintHandling::Label);
        assert_eq!(h.state().print_draft.paper, LABEL_PAPER, "Label puts label stock in");
        h.get_by_role_and_label(Role::Button, tab).click();
        h.run_steps(2);
        assert_eq!(h.state().print_draft.handling, mode);
        assert_eq!(h.state().print_draft.paper, a4, "{tab} gives A4 back after Label");
        h.get_by_value("A4");
    }
}

/// A paper picked while in Label is the person's choice, and leaving Label keeps it.
#[test]
fn a_paper_picked_while_in_label_is_kept_when_leaving_it() {
    use pdfcraft_engine::print::LABEL_PAPER;
    use pdfcraft_ui_egui::{PrintDraft, PrintHandling};
    let (a4, letter) = (paper("A4"), paper("US Letter"));
    let mut d = PrintDraft { paper: a4, ..Default::default() };
    d.set_handling(PrintHandling::Label);
    assert_eq!(d.paper, LABEL_PAPER);
    d.paper = letter;
    d.set_handling(PrintHandling::Size);
    assert_eq!(d.paper, letter);
    // A second round trip from the kept paper gives that paper back.
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Poster);
    assert_eq!(d.paper, letter);
    // Clicking Label again while in Label doesn't forget the paper it replaced.
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Multiple);
    assert_eq!(d.paper, letter);
}

/// Picking the label stock from the Paper menu switches to Label (the existing behaviour); leaving
/// Label afterwards gives back the paper picked before it.
#[test]
fn label_stock_picked_from_the_paper_menu_is_undone_by_leaving_label() {
    use pdfcraft_engine::print::{LABEL_PAPER, PAPERS};
    use pdfcraft_ui_egui::PrintHandling;
    let a4 = paper("A4");
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.state_mut().print_draft.paper = a4;
    h.run_steps(2);
    h.get_by_value("A4").click();
    h.run_steps(2);
    // The label stock is last in a menu that scrolls: bring it into view (it settles over a few
    // frames) before clicking it.
    h.get_by_label(PAPERS[LABEL_PAPER].0).scroll_to_me();
    h.run_steps(4);
    h.get_by_label(PAPERS[LABEL_PAPER].0).click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.paper, LABEL_PAPER);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Label);
    h.get_by_role_and_label(Role::Button, "Size").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Size);
    assert_eq!(h.state().print_draft.paper, a4);
}

/// The same through the Paper menu: US Letter picked while in Label stays when Size is chosen,
/// rather than the A4 Label replaced coming back.
#[test]
fn a_paper_picked_from_the_menu_while_in_label_is_kept() {
    use pdfcraft_engine::print::{LABEL_PAPER, PAPERS};
    use pdfcraft_ui_egui::PrintHandling;
    let (a4, letter) = (paper("A4"), paper("US Letter"));
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.state_mut().print_draft.paper = a4;
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Label").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.paper, LABEL_PAPER);
    h.get_by_value(PAPERS[LABEL_PAPER].0).click();
    h.run_steps(2);
    h.get_by_label("US Letter").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("US Letter").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.paper, letter);
    h.get_by_role_and_label(Role::Button, "Size").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Size);
    assert_eq!(h.state().print_draft.paper, letter, "the paper picked in Label is kept");
    h.get_by_value("US Letter");
}

/// Properties… reads the printer's driver options off the GUI thread: the dialog keeps drawing
/// while CUPS answers, and a read that dies leaves the panel with nothing rather than waiting
/// (#1818). The click's request is made directly, so the test runs where Properties… opens the
/// driver's own window instead (Windows).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn printer_options_are_read_off_the_gui_thread_and_a_dead_read_offers_nothing() {
    use pdfcraft_engine::print::spool::PrinterOption;
    use std::sync::{Arc, Mutex};
    let mut h = harness();
    // The source waits until the test lets it answer, so the dialog is seen while it reads.
    let gate = Arc::new(Mutex::new(()));
    let hold = gate.lock().unwrap();
    let wait = gate.clone();
    h.state_mut().options_source = Some(Arc::new(move |printer: &str| {
        assert_ne!(printer, "Dead", "the read died");
        let _open = wait.lock();
        vec![PrinterOption {
            key: "InputSlot".into(),
            label: "Paper tray".into(),
            group: String::new(),
            choices: vec![("Auto".into(), "Automatic".into()), ("Tray2".into(), "Tray 2".into())],
            default: "Auto".into(),
        }]
    }));
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    let ask = |h: &mut Harness<'static, PdfCraftApp>, printer: &str| {
        let d = &mut h.state_mut().print_draft;
        d.driver_options = None;
        d.driver_options_loading = Some(printer.into());
        d.read_options_for = Some(printer.into());
        d.show_driver_options = true;
    };
    let settle = |h: &mut Harness<'static, PdfCraftApp>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.state().print_draft.driver_options_loading.is_some() && std::time::Instant::now() < deadline {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    ask(&mut h, "Office");
    h.run_steps(3);
    assert_eq!(h.state().print_draft.driver_options_loading.as_deref(), Some("Office"), "frames keep coming while the read waits");
    assert!(h.state().print_draft.driver_options.is_none());
    h.get_by_label("Reading the printer's settings…");
    drop(hold);
    settle(&mut h);
    let (read_for, options) = h.state().print_draft.driver_options.clone().expect("the options arrived");
    assert_eq!(read_for, "Office");
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].key, "InputSlot");
    h.get_by_label("Paper tray");

    ask(&mut h, "Dead");
    settle(&mut h);
    assert_eq!(h.state().print_draft.driver_options, Some(("Dead".into(), Vec::new())), "a dead read offers nothing, and the panel says so");
    h.get_by_label("This printer offers no settings of its own.");
}

/// A read of the printer's options that dies still asks for a frame, so an idle dialog stops
/// saying it is reading without waiting for the mouse to move (#2108 review).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_dead_options_read_wakes_an_idle_dialog() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    let mut h = harness();
    let gate = Arc::new(Mutex::new(()));
    let hold = gate.lock().unwrap();
    let wait = gate.clone();
    let dying = Arc::new(AtomicBool::new(false));
    let flag = dying.clone();
    h.state_mut().options_source = Some(Arc::new(move |_: &str| {
        let _open = wait.lock();
        flag.store(true, Ordering::SeqCst);
        panic!("the read died");
    }));
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    let d = &mut h.state_mut().print_draft;
    d.driver_options = None;
    d.driver_options_loading = Some("Office".into());
    d.read_options_for = Some("Office".into());
    d.show_driver_options = true;
    // The preview's page rasters render on the pool and ask for frames until they arrive; on a
    // loaded machine that outlasts `run`'s step budget, so the dialog is idle only after them.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().render_pending() && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run();
    assert!(!h.ctx.has_requested_repaint(), "the dialog is idle while the read waits");
    drop(hold);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(dying.load(Ordering::SeqCst) && h.ctx.has_requested_repaint()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.ctx.has_requested_repaint(), "the dead read asked for a frame");
    h.run();
    assert_eq!(h.state().print_draft.driver_options, Some(("Office".into(), Vec::new())));
}

/// A document whose security forbids printing (#2669) says so before the dialog opens, and the
/// top bar's Print button is disabled, instead of the dialog refusing on confirm.
#[test]
fn print_restricted_documents_say_so_before_the_dialog() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(include_bytes!("../data/form.pdf").to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b100,
        encrypt_metadata: true,
        seed: [3; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("restricted.pdf", None, bytes).unwrap();
        app
    });
    h.ctx.set_os(egui::os::OperatingSystem::Windows);
    h.run_steps(4);
    assert!(h.get_by_label("Print (Ctrl+P)").accesskit_node().is_disabled());
    h.get_by_label("Print (Ctrl+P)").hover();
    h.run_steps(30);
    h.get_by_label_contains("don't allow printing");
    assert!(!h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("The document's security settings don't allow printing"));
}

/// With no document open the top bar's Print button is disabled too, but not for security
/// reasons: its tooltip must not blame the document's settings (#2669).
#[test]
fn print_without_a_document_does_not_blame_security() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.ctx.set_os(egui::os::OperatingSystem::Windows);
    h.run_steps(4);
    assert!(h.get_by_label("Print (Ctrl+P)").accesskit_node().is_disabled());
    h.get_by_label("Print (Ctrl+P)").hover();
    h.run_steps(30);
    assert!(h.query_by_label_contains("don't allow printing").is_none());
}

#[test]
fn paper_choice_survives_cancel_and_settings_restore() {
    use pdfcraft_engine::print::{Orientation, PAPERS};
    let a4 = PAPERS.iter().position(|(name, _)| *name == "A4").unwrap();
    let letter = PAPERS.iter().position(|(name, _)| *name == "US Letter").unwrap();
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.get_by_value("US Letter").click();
    h.run_steps(2);
    h.get_by_label("A4").click();
    h.run_steps(2);
    assert_eq!(h.state().print_draft.paper, a4);
    // Only paper should be added to persistence; these choices still belong to this session.
    let draft = &mut h.state_mut().print_draft;
    draft.copies = 5;
    draft.which = pdfcraft_ui_egui::PrintWhich::Range;
    draft.range = "1".into();
    draft.orientation = Orientation::Landscape;
    draft.grayscale = true;
    let before_cancel = draft.clone();
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert_eq!(h.state().print_draft, before_cancel, "cancel keeps the existing draft behavior");
    let saved = h.state().persist();
    let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(json["print_paper"], "A4", "store the canonical name, not its position");
    let mut app = PdfCraftApp::new();
    app.restore(&saved);
    let expected = pdfcraft_ui_egui::PrintDraft { paper: a4, ..Default::default() };
    assert_eq!(app.print_draft, expected, "no other print preference becomes persistent");
    let mut restarted = harness_with_settings(Some(saved));
    assert!(restarted.state_mut().execute("print.dialog"));
    restarted.run_steps(3);
    restarted.get_by_value("A4");
    assert_eq!(restarted.state().print_draft.paper, a4);
    restarted.get_by_value("A4").click();
    restarted.run_steps(2);
    restarted.get_by_label("US Letter").click();
    restarted.run_steps(2);
    restarted.get_by_label("Cancel").click();
    restarted.run_steps(2);
    let saved = restarted.state().persist();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&saved).unwrap()["print_paper"], "US Letter");
    let mut again = harness_with_settings(Some(saved));
    assert!(again.state_mut().execute("print.dialog"));
    again.run_steps(3);
    again.get_by_value("US Letter");
    assert_eq!(again.state().print_draft.paper, letter);
}

#[test]
fn paper_restore_accepts_canonical_names_and_ignores_invalid_settings() {
    use pdfcraft_engine::print::PAPERS;
    for (index, (name, _)) in PAPERS.iter().enumerate() {
        let mut app = PdfCraftApp::new();
        app.restore(&serde_json::json!({"print_paper": name}).to_string());
        assert_eq!(app.print_draft.paper, index);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&app.persist()).unwrap()["print_paper"], *name);
    }
    for invalid in [
        "not JSON",
        "{}",
        r#"{"print_paper":null}"#,
        r#"{"print_paper":true}"#,
        r#"{"print_paper":[]}"#,
        r#"{"print_paper":{}}"#,
        r#"{"print_paper":"unknown"}"#,
        r#"{"print_paper":"a4"}"#,
        r#"{"print_paper":4}"#,
        r#"{"print_paper":-1}"#,
        r#"{"print_paper":4294967296}"#,
        r#"{"print_paper":18446744073709551615}"#,
        r#"{"print_paper":1e100}"#,
    ] {
        let mut app = PdfCraftApp::new();
        let factory = app.print_draft.paper;
        app.restore(invalid);
        assert_eq!(app.print_draft.paper, factory, "{invalid}");
        app.restore(r#"{"print_paper":"A4"}"#);
        let selected = app.print_draft.paper;
        app.restore(invalid);
        assert_eq!(app.print_draft.paper, selected, "invalid input must not replace a valid choice: {invalid}");
    }
    let mut app = PdfCraftApp::new();
    app.print_draft.paper = usize::MAX;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&app.persist()).unwrap()["print_paper"], serde_json::Value::Null);
}

#[test]
fn restored_paper_keeps_preview_and_saved_sheet_dimensions() {
    use pdfcraft_engine::print::{self, PAPERS};
    let a4 = PAPERS.iter().find(|(name, _)| *name == "A4").unwrap().1;
    let mut h = harness_with_settings(Some(r#"{"print_paper":"A4"}"#.into()));
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.get_by_value("A4");
    h.get_by_label("Sheet 1 of 1");
    let id = h.state().views[h.state().active.unwrap()].id;
    let doc = h.state().session.get(id).unwrap();
    let sizes: Vec<_> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
    let labels: Vec<_> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let settings = h.state().print_draft.settings(sizes.len(), &labels).unwrap();
    assert_eq!(settings.paper, a4);
    let sheets = print::layout(&sizes, &settings).unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].size, a4, "preview uses the restored paper");
    let out = std::env::temp_dir().join(format!("pdfcraft-restored-paper-{}.pdf", std::process::id()));
    h.state_mut().print_draft.printer = None;
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.run_steps(1);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let printed = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(&out).unwrap())).unwrap();
    let pages = pdfcraft_model::pages(&printed);
    assert_eq!(pages.len(), sheets.len());
    let size = pages[0].display_size(&printed);
    assert!((size.0 - a4.0).abs() < 0.01 && (size.1 - a4.1).abs() < 0.01, "saved sheet: {size:?}");
    std::fs::remove_file(out).unwrap();
}

/// #2707: a booklet decides two-sided itself, as Acrobat does (its Print dialog hides Two-sided
/// while Booklet is chosen): the job flips on the short edge for both sides and prints
/// one-sided for the front or back only, and the Two-sided choice is kept for other layouts.
#[test]
fn a_booklet_decides_two_sided_itself() {
    use pdfcraft_engine::print::{BookletSubset, spool::Duplex};
    let mut h = harness();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(3);
    h.state_mut().print_draft.duplex = Some(Duplex::LongEdge);
    h.run_steps(1);
    assert!(h.query_by_label("Two-sided:").is_some());
    h.get_by_label("Booklet").click();
    h.run_steps(2);
    assert!(h.query_by_label("Two-sided:").is_none(), "hidden while Booklet is chosen");
    let d = &h.state().print_draft;
    assert_eq!(d.duplex, Some(Duplex::LongEdge), "the Two-sided choice is kept");
    assert_eq!(d.job("x").duplex, Some(Duplex::ShortEdge), "both sides flip on the short edge");
    for subset in [BookletSubset::FrontOnly, BookletSubset::BackOnly] {
        h.state_mut().print_draft.booklet_subset = subset;
        assert_eq!(h.state().print_draft.job("x").duplex, Some(Duplex::Off), "{subset:?} is one pass of a manual duplex");
    }
    h.get_by_label("Size").click();
    h.run_steps(2);
    assert!(h.query_by_label("Two-sided:").is_some());
    assert_eq!(h.state().print_draft.job("x").duplex, Some(Duplex::LongEdge), "back to the choice");
}
