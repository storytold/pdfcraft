//! Edit a PDF ▸ the format bar: text formatting floats on the page beside what it formats, so the Edit panel's
//! tool list doesn't move when text is added, selected or edited. Hidden from its More menu, Format text goes back
//! to the panel.

use egui::{Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::AddedContent;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support::AppHarness as _;
use crate::support::input::double_click;

/// The Add content fixture (a 300 × 400 page) at 150 %, with Edit a PDF open.
fn harness() -> Harness<'static, PdfCraftApp> {
    harness_bytes(include_bytes!("../data/form.pdf").to_vec())
}

fn harness_bytes(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    harness_with(bytes, None)
}

/// [`harness_bytes`] on an app restored from `saved` settings, as after a restart.
fn harness_with(bytes: Vec<u8>, saved: Option<pdfcraft_ui_egui::settings::PdfCraftSettings>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        if let Some(saved) = &saved {
            app.apply_settings(saved);
        }
        let ctx = egui::Context::default();
        app.set_option(&ctx, "language", "en").unwrap();
        app.open_bytes(&ctx, "form.pdf", None, bytes.clone()).unwrap();
        app.set_option(&ctx, "zoom", "150").unwrap();
        app.set_option(&ctx, "tool", "edit").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// A 300 × 400 page whose one paragraph reads "Quarterly report" (18 pt Helvetica at 40, 300).
fn paragraph_pdf() -> Vec<u8> {
    let body = "BT /F1 18 Tf 40 300 Td (Quarterly report) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
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

/// A point on the page, from its top left in page units.
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
    s.session.get(s.views[0].id).unwrap().added().to_vec()
}

/// The Edit panel's Text row (Add content).
fn text_row(h: &Harness<'static, PdfCraftApp>) -> Rect {
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    h.query_all_by_label("Text").map(|n| n.rect()).find(|r| r.right() < page.left()).expect("the panel's Text row")
}

/// The control named `label` that floats over the document: right of the Edit panel's rows.
fn on_page(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<Rect> {
    let panel = text_row(h).right();
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| r.left() > panel)
}

/// The control named `label` in the Edit panel.
fn in_panel(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<Rect> {
    let panel = text_row(h).right();
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| r.right() <= panel)
}

fn press(h: &mut Harness<'static, PdfCraftApp>, r: Rect) {
    click(h, r.center());
}

#[test]
fn adding_text_keeps_the_tool_list_still_and_formats_from_the_page() {
    let mut h = harness();
    let before = text_row(&h);
    assert!(h.execute("edit.text"));
    h.run_steps(4);
    assert_eq!(text_row(&h), before, "the Add content list doesn't move");
    assert!(in_panel(&h, "Bold").is_none(), "no Format text section in the panel");
    // The style the next text gets, from the bar.
    let bold = on_page(&h, "Bold").expect("Bold on the format bar");
    press(&mut h, bold);
    assert!(h.state().text_style.bold, "the next text will be bold");
}

#[test]
fn formatting_while_typing_styles_the_box_and_keeps_it_open() {
    let mut h = harness();
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Hi".into()));
    h.run_steps(2);
    let editor = h.get_by_label("Added text").rect();
    let bold = on_page(&h, "Bold").expect("Bold on the format bar");
    assert!(bold.bottom() <= editor.top(), "the bar stays clear of the box: Bold {bold:?}, box {editor:?}");
    press(&mut h, bold);
    let draft = h.state().views[0].session.content.draft.clone().expect("the box stays open");
    assert!(draft.style.bold, "the box being typed turns bold");
    h.event(egui::Event::Text(" there".into()));
    h.run_steps(2);
    let text = h.state().views[0].session.content.draft.as_ref().map(|d| d.text.clone());
    assert_eq!(text.as_deref(), Some("Hi there"), "typing carries on after the click");
}

#[test]
fn selected_added_text_is_formatted_from_the_bar() {
    let mut h = harness();
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Reviewed".into()));
    h.run_steps(2);
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.content.selected, Some((0, 0)), "the new text is selected");
    let italic = on_page(&h, "Italic").expect("Italic on the format bar");
    press(&mut h, italic);
    let AddedContent::Text(t) = &added(&h)[0].content else { panic!("text") };
    assert!(t.italic);
}

#[test]
fn a_paragraph_stays_open_while_its_bar_formats_it() {
    let mut h = harness_bytes(paragraph_pdf());
    assert!(h.execute("edit.edit_text"));
    h.run_steps(2);
    let p = at(&h, 60.0, 400.0 - 304.0);
    click(&mut h, p);
    assert!(h.state().views[0].session.line_editor.is_some(), "the paragraph opens for typing");
    let underline = on_page(&h, "Underline").expect("Underline on the format bar");
    press(&mut h, underline);
    let ed = h.state().views[0].session.line_editor.clone();
    assert!(ed.is_some(), "a press on the bar is not a click away");
    assert!(ed.is_some_and(|e| e.extras.underline), "the paragraph is underlined");
}

#[test]
fn every_format_text_control_is_one_click_from_the_bar() {
    let mut h = harness_bytes(paragraph_pdf());
    assert!(h.execute("edit.edit_text"));
    h.run_steps(2);
    let p = at(&h, 60.0, 400.0 - 304.0);
    click(&mut h, p);
    assert!(on_page(&h, "Superscript").is_none(), "the bar keeps to the common controls");
    let all = on_page(&h, "All text formatting").expect("the bar opens the full panel");
    press(&mut h, all);
    assert!(on_page(&h, "Superscript").is_some(), "the full Format text opens from the bar");
    assert!(h.state().views[0].session.line_editor.is_some(), "with the paragraph still open");
}

#[test]
fn a_selected_image_gets_its_tools_on_the_bar() {
    let mut h = harness();
    let img = craft_image_codecs::Image::from_u8(30, 20, craft_image_codecs::ChannelLayout::Rgb, vec![90u8; 1800]).unwrap();
    let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().add_image(&ctx, "logo.png".into(), png);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.content.selected, Some((0, 0)), "the image is selected");
    assert!(in_panel(&h, "Rotate clockwise").is_none(), "no Edit image section in the panel");
    let rotate = on_page(&h, "Rotate clockwise").expect("Rotate clockwise on the format bar");
    press(&mut h, rotate);
    let AddedContent::Image(img) = &added(&h)[0].content else { panic!("image") };
    assert_ne!(img.rotation, 0, "the image turned");
}

#[test]
fn hiding_the_bar_puts_format_text_back_in_the_panel() {
    let mut h = harness();
    assert!(h.execute("edit.text"));
    h.run_steps(4);
    let more = on_page(&h, "More options").expect("the bar's More menu");
    press(&mut h, more);
    let hide = h.get_by_label("Hide bar").rect();
    press(&mut h, hide);
    h.run_steps(2);
    assert!(on_page(&h, "Bold").is_none(), "the bar is gone");
    assert!(in_panel(&h, "Bold").is_some(), "Format text is back in the panel");
    // Hidden after a restart too.
    let saved = h.state().settings_snapshot();
    let mut restarted = harness_with(include_bytes!("../data/form.pdf").to_vec(), Some(saved));
    assert!(restarted.execute("edit.text"));
    restarted.run_steps(4);
    assert!(in_panel(&restarted, "Bold").is_some() && on_page(&restarted, "Bold").is_none(), "the bar stays hidden after a restart");
    // And the panel brings it back.
    let show = in_panel(&h, "Show on page").expect("a way back to the bar");
    press(&mut h, show);
    h.run_steps(2);
    assert!(on_page(&h, "Bold").is_some(), "the bar is back");
    assert!(in_panel(&h, "Bold").is_none());
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

#[test]
fn switching_tools_puts_down_the_box_being_typed() {
    for tool in ["select", "hand"] {
        let mut h = harness_bytes(paragraph_pdf());
        assert!(h.execute("edit.text"));
        h.run_steps(2);
        let p = at(&h, 40.0, 300.0);
        click(&mut h, p);
        h.event(egui::Event::Text("Live".into()));
        h.run_steps(2);
        h.set_option("quick", tool).unwrap();
        h.run_steps(3);
        assert!(h.state().views[0].session.content.draft.is_none(), "{tool} puts the box down");
        assert!(added(&h).iter().any(|a| matches!(&a.content, AddedContent::Text(t) if t.text == "Live")), "keeping its text ({tool})");
        assert!(h.query_by_label("Done adding text").is_none(), "with its Done and Discard ({tool})");
    }
}

/// Selected page text under Select, then a double click into added text: the Selection bar and the
/// box's Done and Discard show together, each its own bar.
#[test]
fn retyping_added_text_beside_selected_page_text_shows_both_bars() {
    let mut h = harness_bytes(paragraph_pdf());
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Live".into()));
    h.run_steps(2);
    h.set_option("quick", "select").unwrap();
    h.run_steps(3);
    let (from, to) = (at(&h, 30.0, 96.0), at(&h, 200.0, 104.0));
    drag(&mut h, from, to);
    assert!(h.query_by_label("Add comment").is_some(), "the selected text has its Selection bar");
    let word = at(&h, 48.0, 306.0);
    double_click(&mut h, word);
    assert!(h.state().views[0].session.content.draft.as_ref().is_some_and(|d| d.index.is_some()), "the double click retypes the text");
    assert!(h.query_by_label("Done adding text").is_some(), "the box has its Done and Discard");
    assert!(h.query_by_label("Add comment").is_some(), "beside the Selection bar");
    assert!(h.query_by_label_contains("an internal error").is_none(), "without a caught panic");
}

/// Added text retyped under Select is put down by Hand, as a new box is.
#[test]
fn hand_puts_down_text_retyped_under_select() {
    let mut h = harness_bytes(paragraph_pdf());
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Live".into()));
    h.run_steps(2);
    h.set_option("quick", "select").unwrap();
    h.run_steps(3);
    let word = at(&h, 48.0, 306.0);
    double_click(&mut h, word);
    assert!(h.state().views[0].session.content.draft.as_ref().is_some_and(|d| d.index.is_some()), "Select retypes the text");
    h.event(egui::Event::Text(" now".into()));
    h.run_steps(2);
    h.set_option("quick", "hand").unwrap();
    h.run_steps(3);
    assert!(h.state().views[0].session.content.draft.is_none(), "Hand puts the box down");
    assert!(added(&h).iter().any(|a| matches!(&a.content, AddedContent::Text(t) if t.text.contains("now"))), "keeping what was typed");
    assert!(h.query_by_label("Done adding text").is_none(), "with its Done and Discard");
}

/// Two documents side by side: picking Select while the other pane is active puts the box down in
/// its own document, not in the active one.
#[test]
fn a_box_put_down_in_another_pane_lands_in_its_own_document() {
    let mut h = harness_bytes(paragraph_pdf());
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Live".into()));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().open_bytes(&ctx, "b.pdf", None, paragraph_pdf()).unwrap();
    h.run_steps(2);
    let b = h.state().active.expect("the second document is active");
    h.state_mut().move_tab_to(b, pdfcraft_ui_egui::panes::Place::Second).expect("beside the first");
    h.run_steps(3);
    let (a_id, b_id) = (h.state().views[0].id, h.state().views[b].id);
    assert_ne!(a_id, b_id);
    assert!(h.state().views[0].session.content.draft.is_some(), "the first document's box is still open");
    h.state_mut().activate_place(pdfcraft_ui_egui::panes::Place::Second);
    h.set_option("quick", "select").unwrap();
    h.run_steps(3);
    let typed = |id| {
        let s = h.state();
        s.session.get(id).unwrap().added().iter().any(|a| matches!(&a.content, AddedContent::Text(t) if t.text == "Live"))
    };
    assert!(h.state().views[0].session.content.draft.is_none(), "Select puts the box down");
    assert!(typed(a_id), "in the document it was typed in");
    assert!(!typed(b_id), "not in the active one");
    assert_eq!(h.state().active, Some(b), "which stays active");
}

#[test]
fn switching_tools_closes_the_open_paragraph() {
    for tool in ["select", "hand"] {
        let mut h = harness_bytes(paragraph_pdf());
        assert!(h.execute("edit.edit_text"));
        h.run_steps(2);
        let p = at(&h, 60.0, 400.0 - 304.0);
        click(&mut h, p);
        assert!(h.state().views[0].session.line_editor.is_some(), "the paragraph opens for typing");
        h.set_option("quick", tool).unwrap();
        h.run_steps(3);
        assert!(h.state().views[0].session.line_editor.is_none(), "{tool} closes the paragraph");
        assert!(on_page(&h, "Bold").is_none(), "and its format bar ({tool})");
        // Highlighting across the text, then a double click on it, as with the editor gone.
        let (from, to) = (at(&h, 30.0, 96.0), at(&h, 200.0, 104.0));
        drag(&mut h, from, to);
        double_click(&mut h, p);
    }
}

#[test]
fn the_bar_keeps_its_left_edge_as_its_target_changes() {
    let mut h = harness();
    assert!(h.execute("edit.text"));
    h.run_steps(3);
    let armed = h.get_by_label("Move bar").rect().left();
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Reviewed".into()));
    h.run_steps(3);
    let typing = h.get_by_label("Move bar").rect().left();
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    let selected = h.get_by_label("Move bar").rect().left();
    assert!((typing - armed).abs() < 1.0 && (selected - armed).abs() < 1.0, "no jump sideways: {armed} → {typing} → {selected}");
}
