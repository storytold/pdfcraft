//! Edit a PDF ▸ the floating format bar: formatting for new text, added text, a paragraph being
//! edited and an added image floats over the document beside what it formats, so the tool
//! list in the panel never moves.

use egui::{Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::AddedContent;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The Add content fixture (a 300 × 400 page) at 150 %, Edit a PDF open. Frames are 1/60 s, so
/// two clicks in a row fall inside egui's 0.3 s double-click window.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app.set_option("tool", "edit").unwrap();
        app
    });
    settle(&mut h);
    h
}

/// A 200 × 300 page whose one paragraph reads "Page 1" (24 pt Helvetica at 20, 150).
fn paragraph_pdf() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
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

/// A point on the fixture page (display space, top-left origin) → screen.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

fn added(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::Added> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().added.clone()
}

/// The bar's control called `label`, which must float over the document, not sit in a panel.
fn on_document(h: &Harness<'static, PdfCraftApp>, label: &str) -> Rect {
    let r = h.get_by_label(label).rect();
    let doc = h.state().views[0].viewport_rect();
    assert!(doc.contains_rect(r), "{label} at {r:?} should float over the document {doc:?}");
    r
}

/// A tool row in the left panel (the panel is under 300 points wide).
fn panel_row<'a>(h: &'a Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by_label(label).find(|n| n.rect().right() < 300.0).unwrap_or_else(|| panic!("no panel row {label:?}"))
}

#[test]
fn arming_the_text_tool_keeps_the_panel_still_and_marks_the_tool() {
    let mut h = harness();
    let before = panel_row(&h, "Text").rect();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    let row = panel_row(&h, "Text");
    assert_eq!(row.rect(), before, "the tool list doesn't move");
    assert_eq!(row.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "the armed tool is marked");
    assert!(h.query_by_label("FORMAT TEXT").is_none(), "no format section in the panel");
    // The style for new text is on the bar; Bold sets it.
    on_document(&h, "Bold");
    h.get_by_label("Bold").click();
    h.run_steps(3);
    assert!(h.state().text_style.bold);
}

#[test]
fn formatting_while_typing_styles_the_box_and_keeps_typing() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Hi".into()));
    h.run_steps(2);
    let draft = h.state().views[0].content.draft.clone().expect("typing");
    // The bar sits just above the box being typed.
    let bold = on_document(&h, "Bold");
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let k = page.width() / 300.0;
    let box_top = page.top() + (400.0 - draft.rect[3] as f32) * k;
    assert!(bold.bottom() <= box_top && box_top - bold.bottom() < 40.0, "Bold {bold:?}, box top {box_top}");
    h.get_by_label("Bold").click();
    h.run_steps(3);
    let draft = h.state().views[0].content.draft.clone().expect("the box stays open");
    assert!(draft.style.bold, "the box being typed turns bold");
    assert!(added(&h).is_empty(), "nothing committed yet");
    h.event(egui::Event::Text(" there".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.draft.as_ref().map(|d| d.text.as_str()), Some("Hi there"), "typing carries on");
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    let a = added(&h);
    let AddedContent::Text(t) = &a[0].content else { panic!("text") };
    assert_eq!((t.text.as_str(), t.bold), ("Hi there", true));
}

#[test]
fn selected_added_text_is_formatted_from_the_bar_above_it() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Reviewed".into()));
    h.run_steps(2);
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    assert_eq!(panel_row(&h, "Text").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "the owning tool is marked");
    let italic = on_document(&h, "Italic");
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let r = added(&h)[0].content.rect();
    let top = page.top() + (400.0 - r[3] as f32) * page.width() / 300.0;
    assert!(italic.bottom() <= top, "the bar sits above the selection");
    h.get_by_label("Italic").click();
    h.run_steps(3);
    let AddedContent::Text(t) = &added(&h)[0].content else { panic!("text") };
    assert!(t.italic);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Edit content"));
}

#[test]
fn bold_while_editing_a_paragraph_keeps_the_editor_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc.pdf", None, paragraph_pdf()).unwrap();
        app
    });
    settle(&mut h);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let p = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    click(&mut h, p);
    assert!(h.state().views[0].line_editor.is_some());
    assert_eq!(panel_row(&h, "Edit text & images").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    on_document(&h, "Bold");
    h.get_by_label("Bold").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].base_font, "Helvetica-Bold");
    assert!(s.views[0].line_editor.is_some(), "a click on the bar is not a click away");
    // Underline (a paragraph-only control) is on the bar too.
    on_document(&h, "Underline");
    h.get_by_label("Underline").click();
    h.run_steps(4);
    assert!(h.state().views[0].line_editor.as_ref().is_some_and(|e| e.extras.underline));
}

#[test]
fn a_selected_image_gets_its_tools_on_the_bar() {
    let mut h = harness();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 30, 20);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[90u8; 1800]).unwrap();
    }
    h.state_mut().add_image("logo.png".into(), png);
    h.run_steps(4);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    assert_eq!(panel_row(&h, "Image").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    on_document(&h, "Rotate clockwise");
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let AddedContent::Image(img) = &added(&h)[0].content else { panic!("image") };
    assert_eq!(img.rotation, 3);
}

#[test]
fn the_grip_detaches_the_bar_and_a_double_click_docks_it_again() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    let docked = on_document(&h, "Bold");
    let grip = h.get_by_label("Move the format bar").rect().center();
    let to = grip + egui::vec2(120.0, 260.0);
    h.hover_at(grip);
    h.run_steps(1);
    h.drag_at(grip);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(grip + (to - grip) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
    let moved = on_document(&h, "Bold");
    assert!((moved.center() - docked.center() - egui::vec2(120.0, 260.0)).length() < 2.0, "{docked:?} → {moved:?}");
    // Typing a box elsewhere doesn't pull a detached bar back.
    let p = at(&h, 40.0, 60.0);
    click(&mut h, p);
    assert_eq!(on_document(&h, "Bold"), moved);
    // A double-click on the grip docks it again: it follows the box.
    let grip = h.get_by_label("Move the format bar").rect().center();
    h.hover_at(grip);
    h.run_steps(1);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: grip, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        }
        h.run_steps(1);
    }
    h.run_steps(3);
    assert_ne!(on_document(&h, "Bold"), moved, "docked again");
}

#[test]
fn read_mode_shows_no_bar() {
    let mut h = harness();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 30, 20);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[90u8; 1800]).unwrap();
    }
    h.state_mut().add_image("logo.png".into(), png);
    h.run_steps(4);
    on_document(&h, "Rotate clockwise");
    // Read hides the tool panel; its tools' bar goes with it.
    h.state_mut().set_option("mode", "read").unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Rotate clockwise").is_none());
}
