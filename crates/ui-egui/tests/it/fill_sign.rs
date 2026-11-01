//! Fill & Sign in the real shell (egui_kittest): text, marks, date and a drawn signature.

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::fill_sign::{FillTool, SavedSig};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, QuickTool};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

use crate::gpu;

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_bytes(FIXTURE)
}

fn harness_bytes(fixture: &[u8]) -> Harness<'static, PdfCraftApp> {
    let fixture = fixture.to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, fixture.clone()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app.set_option("author", "Ada").unwrap();
        app
    });
    h.run_steps(5);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let s = h.state();
    let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: s.views[0].page_screen_rect(0).expect("on screen"),
        rot: s.views[0].rotation,
        pw: page.width,
        ph: page.height,
    };
    let p = page.user_to_view(x, y);
    xf.norm_to_screen(p[0] / xf.pw, p[1] / xf.ph)
}

/// The screen point `dx` points right and `dy` points up, as displayed, from the user-space point
/// (`x`, `y`): unlike `at`, offsets don't turn with the page's /Rotate.
fn shown(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32, dx: f32, dy: f32) -> Pos2 {
    let s = h.state();
    let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: s.views[0].page_screen_rect(0).expect("on screen"),
        rot: s.views[0].rotation,
        pw: page.width,
        ph: page.height,
    };
    let p = page.user_to_view(x, y);
    xf.norm_to_screen((p[0] + dx) / xf.pw, (p[1] - dy) / xf.ph)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, x: f32, y: f32) {
    let p = at(h, x, y);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

fn items(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<String>)> {
    let s = h.state();
    let mut v: Vec<_> = s.session.get(s.views[0].id).unwrap().info.annotations.iter().map(|a| (a.subtype.clone(), a.contents.clone())).collect();
    v.sort();
    v
}

/// Page rasters arrive from the render worker independently of kittest's virtual frames.
fn rendered_ink(h: &mut Harness<'static, PdfCraftApp>, x: f32, y: f32) -> image::RgbaImage {
    for _ in 0..50 {
        h.run_steps(2);
        let p = at(h, x, y);
        let pixels = h.render().unwrap();
        if pixels.get_pixel(p.x as u32, p.y as u32).0[..3].iter().all(|v| *v < 60) {
            return pixels;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("signature ink did not render at ({x}, {y})");
}

/// #2392: the Line tool's line runs across the page as displayed, on turned pages too (its box was
/// sized in the page's own axes, so a quarter turn stood it on end).
#[test]
fn the_line_mark_runs_across_turned_pages() {
    for degrees in [0, 90, 180, 270] {
        let mut h = harness();
        {
            let s = h.state_mut();
            let id = s.views[0].id;
            s.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees }).unwrap();
        }
        h.run_steps(4);
        assert!(h.state_mut().execute("sign.fill.line"));
        click(&mut h, 150.0, 350.0);
        let s = h.state();
        let info = &s.session.get(s.views[0].id).unwrap().info;
        let r = info.annotations.last().expect("a line was placed").rect;
        let (a, b) = (info.pages[0].user_to_view(r[0], r[1]), info.pages[0].user_to_view(r[2], r[3]));
        let (w, h) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
        assert!(w > 4.0 * h, "rotation {degrees}: the line's displayed box is {w} × {h}, not across");
    }
}

#[test]
fn text_marks_and_date() {
    let mut h = harness();
    assert!(h.state_mut().execute("sign.fill.check"));
    assert!(matches!(h.state().quick_tool, QuickTool::Fill(_)));
    click(&mut h, 50.0, 350.0);
    click(&mut h, 80.0, 350.0);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check), "checkmarks stay repeatable");
    h.state_mut().execute("sign.fill.date");
    click(&mut h, 50.0, 300.0);
    h.state_mut().execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let v = items(&h);
    assert_eq!(v.len(), 4, "{v:?}");
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref() == Some("Ada Lovelace")));
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref().is_some_and(|c| c.matches('/').count() == 2)), "a date: {v:?}");
    assert!(v.iter().any(|(t, _)| t == "Stamp"));
}

/// The open type box, found as the focused text input.
fn type_box_rect(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.query_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.is_focused()).expect("an open type box").rect()
}

#[test]
fn clicking_elsewhere_with_add_text_keeps_the_typed_text() {
    let mut h = harness();
    h.state_mut().execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(1);
    // Still on Add text: the next click starts a new box, and must keep the first one.
    click(&mut h, 50.0, 150.0);
    h.run_steps(2);
    let v = items(&h);
    assert_eq!(v, vec![("FreeText".to_string(), Some("Ada Lovelace".to_string()))]);
    assert!(h.state().views[0].fill_text.as_ref().is_some_and(|t| t.text.is_empty()), "a new, empty box is open");
}

#[test]
fn fill_text_size_controls_new_text_and_survives_settings_restore() {
    let mut h = harness();
    h.state_mut().execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_label("Font size:");
    h.state_mut().fill_text_size = 24.0;
    click(&mut h, 50.0, 250.0);
    assert_eq!(h.state().views[0].fill_text.as_ref().map(|t| t.size), Some(24.0));
    h.event(egui::Event::Text("Larger text".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let annotation =
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Larger text")).unwrap();
    assert!(annotation.rect[3] - annotation.rect[1] > 24.0, "24 pt text has a larger annotation: {annotation:?}");
    let settings = h.state().persist();
    let mut restored = PdfCraftApp::new();
    restored.restore(&settings);
    assert_eq!(restored.fill_text_size, 24.0);
    restored.restore(r#"{"fill_text_size":1000000}"#);
    assert_eq!(restored.fill_text_size, 24.0, "invalid settings do not change the size");
}

/// #1274: with a placed Fill & Sign text selected, the Font size control resizes that text,
/// not only text typed afterwards; with nothing selected it changes nothing on the page.
#[test]
fn fill_text_size_resizes_the_selected_text() {
    let mut h = harness();
    h.state_mut().execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Resize me".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let height = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Resize me")).cloned().unwrap();
        a.rect[3] - a.rect[1]
    };
    let before = height(&h);
    let set_size = |h: &mut Harness<'static, PdfCraftApp>, value: &str| {
        h.query_all_by_role(egui::accesskit::Role::SpinButton)
            .find(|n| n.value().is_some_and(|v| v.ends_with(" pt")))
            .expect("the Font size control")
            .click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text(value.into()));
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
    };
    // Nothing selected: only the size for new text changes.
    h.state_mut().views[0].comments.selected = None;
    set_size(&mut h, "18");
    assert_eq!(h.state().fill_text_size, 18.0);
    assert_eq!(height(&h), before, "an unselected placed text keeps its size");
    // The placed text picked up with the Add text tool, then resized.
    let annotation = {
        let s = h.state();
        s.session
            .get(s.views[0].id)
            .unwrap()
            .info
            .annotations
            .iter()
            .find(|a| a.contents.as_deref() == Some("Resize me"))
            .map(|a| (a.page, a.index))
            .unwrap()
    };
    h.state_mut().views[0].comments.selected = Some(annotation);
    set_size(&mut h, "36");
    assert_eq!(h.state().fill_text_size, 36.0);
    let after = height(&h);
    assert!(after > before * 2.0, "36 pt text has a taller box than 10 pt text: {before} -> {after}");
}

/// The font sizes of the texts `text` drawn in the last frame.
fn drawn_sizes(h: &Harness<'static, PdfCraftApp>, text: &str) -> Vec<f32> {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Vec<f32>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            egui::Shape::Text(t) if t.galley.text() == text => {
                out.extend(t.galley.job.sections.first().map(|s| s.format.font_id.size));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &h.output().shapes {
        walk(&clipped.shape, text, &mut out);
    }
    out
}

/// The type box draws its text at the page's zoom whichever way the view is turned. With the
/// view turned a quarter, the page's width runs down the screen: reading the zoom off the screen
/// rectangle's width instead made the typed text a height-over-width too large (here 4/3).
#[test]
fn the_type_box_text_matches_the_page_zoom_in_a_turned_view() {
    for rotate in ["0", "90"] {
        let mut h = harness();
        h.state_mut().set_option("rotate", rotate).unwrap();
        h.run_steps(3);
        h.state_mut().execute("sign.fill.text");
        click(&mut h, 50.0, 250.0);
        h.event(egui::Event::Text("Ada".into()));
        h.run_steps(2);
        let s = h.state();
        let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
        let rect = s.views[0].page_screen_rect(0).expect("on screen");
        // Screen pixels per page point: the page's width lies across, or down when turned.
        let zoom = if rotate == "90" { rect.height() } else { rect.width() } / page.width;
        let size = s.views[0].fill_text.as_ref().expect("an open type box").size as f32;
        let drawn = drawn_sizes(&h, "Ada");
        assert!(!drawn.is_empty(), "{rotate}°: the typed text is drawn");
        for d in drawn {
            assert!((d - size * zoom).abs() < 0.5, "{rotate}°: drawn at {d}, the page shows it at {}", size * zoom);
        }
    }
}

#[test]
fn fill_text_colour_survives_preview_commit_settings_and_save() {
    let mut h = harness();
    h.state_mut().execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(2);
    let color = h.state().fill_text_color;
    assert!(color[2] > color[0], "blue was selected: {color:?}");
    click(&mut h, 50.0, 250.0);
    assert_eq!(h.state().views[0].fill_text.as_ref().unwrap().color, color, "the editor uses the selected colour");
    h.event(egui::Event::Text("Blue form text".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let id = h.state().views[0].id;
    let expected = color.map(|v| v as f32);
    let annotation = h.state().session.get(id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Blue form text")).unwrap();
    assert_eq!(annotation.color, Some(expected));
    let bytes = h.state().session.save_bytes(id).unwrap();
    let reopened = harness_bytes(&bytes);
    let saved = reopened
        .state()
        .session
        .get(reopened.state().views[0].id)
        .unwrap()
        .info
        .annotations
        .iter()
        .find(|a| a.contents.as_deref() == Some("Blue form text"))
        .unwrap();
    assert_eq!(saved.color, Some(expected));
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert_eq!(restored.fill_text_color, color);
    restored.restore(r#"{"fill_text_color":[2,0,0]}"#);
    assert_eq!(restored.fill_text_color, color, "invalid saved colours are ignored");
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert!(!items(&h).iter().any(|(_, text)| text.as_deref() == Some("Blue form text")));
    h.state_mut().execute("edit.redo");
    h.run_steps(2);
    assert!(items(&h).iter().any(|(_, text)| text.as_deref() == Some("Blue form text")));
}

#[test]
fn the_type_box_grows_as_text_is_typed() {
    let mut h = harness();
    h.state_mut().execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.run_steps(2);
    let empty = type_box_rect(&h).width();
    h.event(egui::Event::Text("Ada Lovelace, Countess of Lovelace".into()));
    h.run_steps(3);
    let full = type_box_rect(&h).width();
    assert!(full > empty * 2.0, "the box widens with its text: {empty} -> {full}");
}

#[test]
fn signing_draws_a_signature_once_and_places_it() {
    let mut h = harness();
    assert!(h.state_mut().execute("sign.fill.signature"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Signature), "no signature yet: the pad opens");
    h.get_all_by_label("Draw").last().unwrap().click();
    h.run_steps(2);
    let pad = h.get_by_label("Draw your signature below.").rect();
    let start = pos2(pad.left() + 40.0, pad.bottom() + 70.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    for k in 1..=8 {
        h.hover_at(start + egui::vec2(k as f32 * 30.0, if k % 2 == 0 { -20.0 } else { 20.0 }));
        h.run_steps(1);
    }
    h.drop_at(start + egui::vec2(240.0, 0.0));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.state().signature.is_some());
    assert_eq!(h.state().dialog, None);
    click(&mut h, 60.0, 100.0);
    assert!(items(&h).iter().any(|(t, _)| t == "Ink"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    // The signature is remembered (persisted with the app's settings).
    let saved = h.state().persist();
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&saved);
    assert!(again.signature.is_some());
}

#[test]
fn typed_signatures_and_initials() {
    let mut h = harness();
    h.state_mut().comment_prefs.author = "Grace Hopper".into();
    assert!(h.state_mut().execute("sign.fill.signature"));
    h.run_steps(2);
    // Type is the default, with the author's name filled in.
    h.get_by_label("Type your signature.");
    assert_eq!(h.state().signature_draft.text, "Grace Hopper");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(pdfcraft_ui_egui::fill_sign::SavedSig::Typed("Grace Hopper".into())));
    click(&mut h, 60.0, 100.0);
    assert!(items(&h).iter().any(|(t, _)| t == "Stamp"), "typed signatures are filled outlines");
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Initials: their own pad (GH), then placed.
    assert!(h.state_mut().execute("sign.fill.initials"));
    h.run_steps(2);
    h.get_by_label("Create initials");
    assert_eq!(h.state().signature_draft.text, "GH");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    click(&mut h, 60.0, 160.0);
    assert_eq!(items(&h).iter().filter(|(t, _)| t == "Stamp").count(), 2);
    // Both are remembered.
    let saved = h.state().persist();
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&saved);
    assert_eq!(again.signature, h.state().signature);
    assert_eq!(again.initials, Some(pdfcraft_ui_egui::fill_sign::SavedSig::Typed("GH".into())));
}

#[test]
fn changing_saved_signatures_and_initials_preserves_placed_marks() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.state_mut().execute("sign.fill.signature");
    h.run_steps(2);
    assert_eq!(h.state().dialog, None, "Sign still reuses the saved signature");
    click(&mut h, 40.0, 300.0);
    let original = format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations);

    // The quick toolbar exposes replacement without changing the normal placement action.
    h.get_by_label("Fill & Sign").click();
    h.run_steps(2);
    h.get_by_label("Fill & Sign").click();
    h.run_steps(2);
    h.get_by_label("Change signature").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Signature));
    assert_eq!(h.state().signature_draft.text, "Ada Lovelace");
    h.get_by_label("Type your signature.").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Grace Hopper".into()));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(h.state().initials, Some(SavedSig::Typed("AL".into())));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Signature));
    assert_eq!(format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations), original);
    click(&mut h, 40.0, 200.0);
    assert_eq!(items(&h).len(), 2);

    h.get_by_label("Fill & Sign").click();
    h.run_steps(2);
    h.get_by_label("Fill & Sign").click();
    h.run_steps(2);
    h.get_by_label("Change initials").click();
    h.run_steps(2);
    assert!(h.state().signature_draft.initials);
    assert_eq!(h.state().signature_draft.text, "AL");
    h.state_mut().signature_draft.text = "GH".into();
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Initials));
    click(&mut h, 40.0, 100.0);
    assert_eq!(items(&h).len(), 3);
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&h.state().persist());
    assert_eq!(again.signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(again.initials, Some(SavedSig::Typed("GH".into())));
}

#[test]
fn undo_button_and_ctrl_z_take_back_one_drawn_stroke_at_a_time() {
    let mut h = harness();
    let drawn = SavedSig::Drawn(vec![vec![[0.1, 0.1], [0.5, 0.2]], vec![[0.2, 0.3], [0.6, 0.1]], vec![[0.7, 0.2], [0.9, 0.3]]]);
    h.state_mut().signature = Some(drawn);
    assert!(h.state_mut().execute("sign.fill.signature.change"));
    h.run_steps(2);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.strokes.len(), 2);
    h.key_press(egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.strokes.len(), 2, "plain Z is not undo");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.strokes.len(), 1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert!(h.state().signature_draft.strokes.is_empty());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert!(h.state().signature_draft.strokes.is_empty());
}

#[test]
fn changing_drawn_signatures_can_be_cancelled_or_switched_to_type() {
    let mut h = harness();
    let drawn = SavedSig::Drawn(vec![vec![[0.1, 0.1], [0.5, 0.2], [0.9, 0.1]]]);
    h.state_mut().signature = Some(drawn.clone());
    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.state_mut().quick_tool = QuickTool::Fill(FillTool::Check);
    assert!(h.state_mut().execute("sign.fill.signature.change"));
    h.run_steps(2);
    assert!(h.state().signature_draft.drawing);
    assert_eq!(h.state().signature_draft.saved(), Some(drawn.clone()));
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(h.state().signature_draft.strokes.is_empty());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(drawn.clone()));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));

    assert!(h.state_mut().execute("sign.fill.signature.change"));
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.saved(), Some(drawn.clone()));
    h.get_all_by_label("Type").last().unwrap().click();
    h.run_steps(2);
    h.state_mut().signature_draft.text = "Grace Hopper".into();
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(h.state().initials, Some(SavedSig::Typed("AL".into())));
}

#[test]
fn saved_signature_cards_remove_and_add_without_changing_the_document() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 300.0);
    let original = format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations);
    h.state_mut().left = pdfcraft_ui_egui::LeftPanel::Tool("fill_sign");
    h.state_mut().left_open = true;
    h.run_steps(3);
    h.get_by_label("Use signature").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None, "the preview reuses the saved value");
    h.get_by_label("Remove saved signature").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, None);
    assert_eq!(h.state().initials, Some(SavedSig::Typed("AL".into())));
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&h.state().persist());
    assert_eq!(again.signature, None, "removal survives restart");
    h.get_by_label("Add signature").click();
    h.run_steps(3);
    h.state_mut().signature_draft.text = "Grace Hopper".into();
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    h.get_by_label("Remove saved initials").click();
    h.run_steps(3);
    assert_eq!(h.state().initials, None);
    h.get_by_label("Add initials").click();
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().initials, None);
    assert_eq!(format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations), original);
}

#[test]
fn long_typed_names_fit_the_placed_signature_and_keep_every_outline() {
    let text = "Alexandria Catherine Elizabeth Montgomery-Wellington";
    let sig = SavedSig::Typed(text.into());
    let page = pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let pdfcraft_engine::Edit::AddAnnotation(a) = pdfcraft_ui_egui::fill_sign::place(0, &page, [40.0, 200.0], &sig, false, "").unwrap() else {
        panic!("expected annotation");
    };
    let pdfcraft_engine::Shape::TypedSignature { rect, contours } = a.shape else { panic!("expected typed signature") };
    assert!((rect[2] - rect[0] - 150.0).abs() < 0.001, "long names shrink to fit: {rect:?}");
    assert_eq!(contours.len(), pdfcraft_engine::script_outline(text).contours.len());
    assert!(contours.iter().flatten().all(|p| p.iter().all(|v| (0.0..=1.0).contains(v))), "all ink stays inside the appearance bounds");
    let mut h = harness();
    h.state_mut().signature = Some(sig.clone());
    h.state_mut().execute("sign.fill.signature.change");
    h.run_steps(3);
    assert_eq!(h.state().signature_draft.text, text);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(sig));
    click(&mut h, 40.0, 200.0);
    let s = h.state();
    let a = &s.session.get(s.views[0].id).unwrap().info.annotations[0];
    assert!(a.rect[2] <= 190.001, "the full name fits on the page: {:?}", a.rect);
}

fn signature_file(test: &str) -> std::path::PathBuf {
    signature_file_with_mark(test, 10..20)
}

/// A 120 x 40 signature: a stroke across the middle and a block above it at the columns `mark`
/// (the rest is transparent), so a turned or flipped picture shows.
fn signature_file_with_mark(test: &str, mark: std::ops::Range<u32>) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pdfcraft-signature-{test}-{}.png", std::process::id()));
    let image = image::RgbaImage::from_fn(120, 40, |x, y| {
        if (10..110).contains(&x) && (15..25).contains(&y) || mark.contains(&x) && (5..15).contains(&y) {
            image::Rgba([20, 30, 40, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    image.save(&path).unwrap();
    path
}

fn browse_image(h: &mut Harness<'static, PdfCraftApp>, path: &std::path::Path) {
    h.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    h.get_by_label("Browse…").click();
    h.run_steps(3);
}

#[test]
fn image_signatures_and_initials_can_be_imported_placed_and_remembered() {
    let _gpu = gpu();
    let path = signature_file("import");
    let mut h = harness();
    for (command, y, initials) in [("sign.fill.signature", 250.0, false), ("sign.fill.initials", 150.0, true)] {
        h.state_mut().execute(command);
        h.run_steps(2);
        h.get_by_label("Image").click();
        h.run_steps(2);
        assert!(h.state().signature_draft.saved().is_none(), "Apply needs an image");
        browse_image(&mut h, &path);
        assert!(h.state().signature_draft.image.is_some());
        if !initials && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            h.render().unwrap().save(format!("{dir}/image-signature-dialog.png")).unwrap();
        }
        assert_eq!(items(&h).len(), usize::from(initials), "importing doesn't edit the PDF");
        h.get_by_label("Apply").click();
        h.run_steps(3);
        click(&mut h, 40.0, y);
        assert_eq!(h.state().quick_tool, QuickTool::Select);
        assert_eq!(h.state().views[0].comments.selected, Some((0, usize::from(initials))));
    }
    assert_eq!(items(&h).len(), 2);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    for a in &doc.info.annotations {
        assert_eq!(a.subtype, "Stamp");
        assert!(((a.rect[2] - a.rect[0]) / (a.rect[3] - a.rect[1]) - 3.0).abs() < 0.001);
        assert!(a.rect[2] - a.rect[0] <= 150.0);
    }
    assert_eq!(doc.can_undo(), Some("Add initials"));
    let settings = state.persist();
    assert!(!settings.contains(&path.to_string_lossy().to_string()), "only the image is saved, never its source path");
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&settings);
    assert_eq!(again.signature, state.signature);
    assert_eq!(again.initials, state.initials);
    let saved_image = state.signature.clone();
    h.state_mut().execute("sign.fill.signature.change");
    h.run_steps(2);
    assert!(h.state().signature_draft.image_mode);
    assert!(h.state().signature_draft.image.is_some());
    h.get_by_label("Clear").click();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, saved_image);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(items(&h).len(), 1);
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!(items(&h).len(), 2);
    // Show the saved preview cards in the actual shell as well as the dialog.
    h.state_mut().set_option("tool", "fill_sign").unwrap();
    h.run_steps(3);
    h.get_by_label("Use signature");
    h.get_by_label("Use initials");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/image-signatures.png")).unwrap();
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_preview_follows_pointer_at_page_size_with_zoom_and_rotation() {
    let _gpu = gpu();
    // The block above the stroke is at its right end, away from the pointer's own cursor.
    let path = signature_file_with_mark("pointer", 100..110);
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(&path).unwrap()).unwrap();
    let mut h = harness();
    // A link under the placement point must not replace the image with a hand cursor.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddLink {
        page: 0,
        rect: [30.0, 230.0, 60.0, 270.0],
        action: pdfcraft_engine::LinkAction::Page(0),
        style: pdfcraft_engine::LinkStyle::default(),
    });
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.state_mut().execute("sign.fill.signature");
    // Exercise document rotation and view rotation separately and together. The image is upright
    // as displayed, whatever the page's /Rotate; the view rotation turns the whole screen.
    let mut previous_document_rotation = 0;
    for (document_rotation, view_rotation, zoom) in [(0, 0, "150"), (90, 0, "100"), (0, 90, "100"), (90, 90, "75"), (0, 180, "75"), (0, 270, "100")] {
        let delta = document_rotation - previous_document_rotation;
        if delta != 0 {
            assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: delta }));
        }
        previous_document_rotation = document_rotation;
        while h.state().views[0].rotation != view_rotation {
            h.state_mut().views[0].rotate_view(true);
        }
        h.state_mut().set_option("zoom", zoom).unwrap();
        h.run_steps(3);
        for y in [250.0, 150.0] {
            h.hover_at(at(&h, 40.0, y));
            h.run_steps(2);
            assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "the image replaces the crosshair");
            // The picture is 96 x 32 pt with its left edge at the pointer: the stroke spans 8..88
            // and the block above it 80..88. egui_kittest paints a mouse cursor, a 16 px triangle
            // down and right of the pointer, over the render, so only probe far from the pointer.
            let ink = shown(&h, 40.0, y, 50.0, 0.0);
            let margin = shown(&h, 40.0, y, 92.0, 0.0);
            let upper_stroke = shown(&h, 40.0, y, 84.0, 8.0);
            let lower_margin = shown(&h, 40.0, y, 84.0, -8.0);
            let pixels = h.render().unwrap();
            assert!(pixels.get_pixel(ink.x as u32, ink.y as u32).0[..3].iter().all(|v| *v < 60), "ink follows the pointer at the placed size");
            assert!(pixels.get_pixel(margin.x as u32, margin.y as u32).0[..3].iter().all(|v| *v > 240), "alpha exposes the page");
            assert!(
                pixels.get_pixel(upper_stroke.x as u32, upper_stroke.y as u32).0[..3].iter().all(|v| *v < 60),
                "the asymmetric image stays upright as displayed"
            );
            assert!(pixels.get_pixel(lower_margin.x as u32, lower_margin.y as u32).0[..3].iter().all(|v| *v > 240), "the image isn't flipped");
            if y == 150.0 {
                let old = shown(&h, 40.0, 250.0, 50.0, 0.0);
                assert!(pixels.get_pixel(old.x as u32, old.y as u32).0[..3].iter().all(|v| *v > 240), "moving the pointer removes the old preview");
            }
            if document_rotation == 0
                && view_rotation == 0
                && y == 250.0
                && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
            {
                pixels.save(format!("{dir}/image-signature-pointer.png")).unwrap();
            }
        }
        assert!(items(&h).is_empty(), "hovering never adds a signature");
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().info.links.len(), 1, "the original link remains");
    }
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(2);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "restore the pointer off the page");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_placement_selects_resize_handles_and_requires_reselecting_to_repeat() {
    let _gpu = gpu();
    let path = signature_file("resize");
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(&path).unwrap()).unwrap();
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Image(image.clone()));
    h.state_mut().initials = Some(SavedSig::Image(image));
    h.state_mut().comment_prefs.pinned = true;
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "signatures place once even when comment tools are pinned");
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    let placed = rendered_ink(&mut h, 90.0, 250.0);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        placed.save(format!("{dir}/image-signature-selected.png")).unwrap();
    }
    // The bottom-right handle is ready without another selection click.
    let corner = at(&h, 136.0, 234.0);
    h.hover_at(corner);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
    h.drag_at(corner);
    h.run_steps(1);
    let end = corner + egui::vec2(96.0, 32.0);
    h.hover_at(end);
    h.run_steps(2);
    h.drop_at(end);
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(doc.info.annotations[0].rect.iter().zip([40.0, 218.0, 184.0, 266.0]).all(|(a, b)| (*a - b).abs() < 0.001));
    assert_eq!(doc.can_undo(), Some("Resize comment"));
    rendered_ink(&mut h, 160.0, 242.0);
    h.state_mut().undo();
    h.run_steps(2);
    assert!(
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0]
            .rect
            .iter()
            .zip([40.0, 234.0, 136.0, 266.0])
            .all(|(a, b)| (*a - b).abs() < 0.001)
    );
    h.state_mut().redo();
    h.run_steps(2);
    click(&mut h, 40.0, 150.0);
    assert_eq!(items(&h).len(), 1, "a later page click doesn't add another signature");
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 150.0);
    assert_eq!(items(&h).len(), 2, "selecting the saved signature again places another");
    h.state_mut().execute("sign.fill.initials");
    click(&mut h, 40.0, 100.0);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 2)));
    std::fs::remove_file(path).unwrap();
}

fn assert_rect(actual: [f32; 4], expected: [f32; 4]) {
    assert!(actual.iter().zip(expected).all(|(a, b)| (*a - b).abs() < 0.001), "{actual:?} != {expected:?}");
}

fn pixel_at(h: &Harness<'static, PdfCraftApp>, pixels: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    let p = at(h, x, y);
    pixels.get_pixel(p.x.round() as u32, p.y.round() as u32).0
}

fn has_blue_near(pixels: &image::RgbaImage, p: Pos2) -> bool {
    (-5..=5).any(|dy| {
        (-5..=5).any(|dx| {
            let v = pixels.get_pixel((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32).0;
            i32::from(v[2]) > i32::from(v[0]) + 40 && i32::from(v[2]) > i32::from(v[1]) + 35
        })
    })
}

#[test]
fn image_signature_live_resize_and_move_preserve_background_and_hide_moving_handles() {
    let _gpu = gpu();
    // Original synthetic page content; a white patch over the old image would fail this test.
    const GREEN_PAGE: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n4 0 obj << /Length 36 >> stream\n0.8 1 0.7 rg 0 0 300 400 re f\nendstream\nendobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let mut h = harness_bytes(GREEN_PAGE);
    let path = signature_file("live");
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(&path).unwrap()).unwrap();
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    rendered_ink(&mut h, 90.0, 250.0);
    let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();
    let corner = at(&h, 136.0, 234.0);
    h.drag_at(corner);
    h.run_steps(1);
    let end = at(&h, 184.0, 228.0);
    h.hover_at(end);
    // Wait only for the one-time background preparation. Later pointer frames are immediate.
    rendered_ink(&mut h, 160.0, 242.0);
    let end = at(&h, 220.0, 236.0);
    h.hover_at(end);
    h.run_steps(2);
    let resized = h.render().unwrap();
    assert!(pixel_at(&h, &resized, 200.0, 236.0)[..3].iter().all(|v| *v < 60), "ink resizes before release");
    let assert_green = |v: [u8; 4]| {
        assert!(v[1] > 230 && i32::from(v[1]) > i32::from(v[0]) + 10 && i32::from(v[1]) > i32::from(v[2]) + 10, "page content is preserved: {v:?}")
    };
    assert_green(pixel_at(&h, &resized, 90.0, 250.0));
    assert!(has_blue_near(&resized, at(&h, 220.0, 266.0)), "resizing keeps its handles visible");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.edit_generation(), generation, "pointer frames never edit the document");
    assert_eq!(doc.can_undo(), Some("Add signature"));
    assert_rect(doc.info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        resized.save(format!("{dir}/image-signature-live-resize.png")).unwrap();
    }
    h.drop_at(end);
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));

    let start = at(&h, 130.0, 236.0);
    h.drag_at(start);
    h.run_steps(1);
    for (x, y) in [(140.0, 180.0), (150.0, 140.0)] {
        h.hover_at(at(&h, x, y));
        h.run_steps(2);
        let moving = h.render().unwrap();
        assert!(pixel_at(&h, &moving, x, y)[..3].iter().all(|v| *v < 60), "ink follows each pointer position");
        assert_green(pixel_at(&h, &moving, 130.0, 236.0));
        assert_green(pixel_at(&h, &moving, x - 85.0, y));
        for (hx, hy) in [
            (x - 90.0, y + 30.0),
            (x, y + 30.0),
            (x + 90.0, y + 30.0),
            (x + 90.0, y),
            (x + 90.0, y - 30.0),
            (x, y - 30.0),
            (x - 90.0, y - 30.0),
            (x - 90.0, y),
        ] {
            assert!(!has_blue_near(&moving, at(&h, hx, hy)), "moving hides the frame and all eight handles");
        }
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));
        if y == 140.0
            && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
        {
            moving.save(format!("{dir}/image-signature-live-move.png")).unwrap();
        }
    }
    let end = at(&h, 150.0, 140.0);
    h.drop_at(end);
    h.run_steps(2);
    let released = h.render().unwrap();
    assert!(has_blue_near(&released, at(&h, 240.0, 170.0)), "the frame returns immediately on release");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Move comment"));
    assert_eq!(doc.edit_generation(), generation + 2, "one edit for each completed gesture");
    assert_rect(doc.info.annotations[0].rect, [60.0, 110.0, 240.0, 170.0]);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        released.save(format!("{dir}/image-signature-move-released.png")).unwrap();
    }
    h.state_mut().undo();
    h.state_mut().undo();
    h.run_steps(3);
    assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    rendered_ink(&mut h, 90.0, 250.0);
    click(&mut h, 90.0, 250.0);
    h.drag_at(at(&h, 90.0, 250.0));
    h.run_steps(1);
    h.hover_at(at(&h, 150.0, 180.0));
    rendered_ink(&mut h, 150.0, 180.0);
    assert_eq!(
        h.state().session.get(h.state().views[0].id).unwrap().can_undo(),
        Some("Add signature"),
        "moving preview is still read-only before Escape"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].comments.gesture.is_none(), "Escape must cancel before releasing the mouse");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add signature"), "Escape must not commit");
    h.drop_at(at(&h, 150.0, 180.0));
    h.run_steps(2);
    assert!(h.state().views[0].comments.gesture.is_none());
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Add signature"), "Escape cancels without an undo step");
    assert_rect(doc.info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    let cancelled = h.render().unwrap();
    assert_green(pixel_at(&h, &cancelled, 150.0, 180.0));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn embedded_image_signature_live_gestures_follow_document_and_view_rotation() {
    let _gpu = gpu();
    let path = signature_file("live-rotations");
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(&path).unwrap()).unwrap();
    for (document_rotation, view_rotation) in [(90, 0), (0, 90), (90, 270)] {
        let mut h = harness();
        h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: document_rotation });
        while h.state().views[0].rotation != view_rotation {
            h.state_mut().views[0].rotate_view(true);
        }
        h.state_mut().set_option("zoom", "100").unwrap();
        h.state_mut().signature = Some(SavedSig::Image(image.clone()));
        h.state_mut().execute("sign.fill.signature");
        h.run_steps(3);
        // Points as displayed (from the shown page's bottom-left, y up). The signature is upright
        // as displayed, so the same points apply whatever the page's /Rotate.
        let page = h.state().session.get(h.state().views[0].id).unwrap().info.pages[0].clone();
        let u = |x: f32, y: f32| page.view_to_user(x, page.height - y);
        let [x, y] = u(40.0, 250.0);
        click(&mut h, x, y);
        let [x, y] = u(90.0, 250.0);
        rendered_ink(&mut h, x, y);
        // Future placements may use a different signature: drag the PDF's embedded image.
        h.state_mut().signature = None;
        let [x, y] = u(136.0, 234.0);
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let [x, y] = u(184.0, 228.0);
        let end = at(&h, x, y);
        h.hover_at(end);
        let [x, y] = u(160.0, 242.0);
        rendered_ink(&mut h, x, y);
        let pixels = h.render().unwrap();
        let [x, y] = u(58.0, 254.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v < 60), "asymmetric ink stays upright during resize");
        let [x, y] = u(58.0, 230.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v > 240), "alpha isn't flipped");
        h.drop_at(end);
        h.run_steps(3);
        // The corner keeps the image's 3:1 as displayed: 144 x 48 from the top-left corner.
        let ([x0, y0], [x1, y1]) = (u(40.0, 218.0), u(184.0, 266.0));
        let rect = h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect;
        let want = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        assert!(rect.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01), "{rect:?} != {want:?}");
        let [x, y] = u(112.0, 242.0);
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let [x, y] = u(132.0, 142.0);
        let target = at(&h, x, y);
        h.hover_at(target);
        h.run_steps(2);
        let pixels = h.render().unwrap();
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v < 60), "image moves before release");
        let [x, y] = u(160.0, 242.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v > 240), "old ink is removed during movement");
        let [x, y] = u(204.0, 166.0);
        let corner = at(&h, x, y);
        assert!(!has_blue_near(&pixels, corner));
        h.drop_at(target);
        h.run_steps(2);
        assert!(has_blue_near(&h.render().unwrap(), corner));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_corners_restore_original_aspect_after_edge_resize_and_reopen() {
    let _gpu = gpu();
    let path = signature_file("aspect");
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(&path).unwrap()).unwrap();
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 70.0, 210.0);
    // Side handles stretch just their own axis; pointer movement along the other axis is ignored.
    for (from, to, expected) in
        [([166.0, 210.0], [214.0, 216.0], [70.0, 194.0, 214.0, 226.0]), ([142.0, 194.0], [150.0, 170.0], [70.0, 170.0, 214.0, 226.0])]
    {
        h.drag_at(at(&h, from[0], from[1]));
        h.run_steps(1);
        let end = at(&h, to[0], to[1]);
        h.hover_at(end);
        h.run_steps(2);
        h.drop_at(end);
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, expected);
    }
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    // Reopened PDFs have no saved signature setting: each corner must use the embedded 3:1 image,
    // rather than the stretched 144:56 annotation rectangle.
    for (hx, hy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let mut h = harness_bytes(&bytes);
        assert!(h.state().signature.is_none());
        h.state_mut().views[0].comments.selected = Some((0, 0));
        h.run_steps(3);
        rendered_ink(&mut h, 142.0, 198.0);
        let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();
        let (x, y) = (if hx < 0.0 { 70.0 } else { 214.0 }, if hy < 0.0 { 170.0 } else { 226.0 });
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let end = at(&h, x + hx * 10.0, y + hy * 10.0);
        h.hover_at(end);
        let expected = [
            if hx < 0.0 { 16.0 } else { 70.0 },
            if hy < 0.0 { 160.0 } else { 170.0 },
            if hx < 0.0 { 214.0 } else { 268.0 },
            if hy < 0.0 { 226.0 } else { 236.0 },
        ];
        let ink_x = expected[0] + 198.0 * if hx < 0.0 { 0.15 } else { 0.85 };
        let pixels = rendered_ink(&mut h, ink_x, (expected[1] + expected[3]) / 2.0);
        assert!(has_blue_near(&pixels, at(&h, if hx < 0.0 { expected[0] } else { expected[2] }, if hy < 0.0 { expected[1] } else { expected[3] })));
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.edit_generation(), generation, "corner previews remain read-only");
        assert_rect(doc.info.annotations[0].rect, [70.0, 170.0, 214.0, 226.0]);
        if hx > 0.0
            && hy < 0.0
            && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
        {
            pixels.save(format!("{dir}/image-signature-aspect-corner.png")).unwrap();
        }
        h.drop_at(end);
        h.run_steps(3);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_rect(doc.info.annotations[0].rect, expected);
        assert_eq!(doc.edit_generation(), generation + 1);
        assert_eq!(doc.can_undo(), Some("Resize comment"));
        assert!(((expected[2] - expected[0]) / (expected[3] - expected[1]) - 3.0).abs() < 0.001);
        h.state_mut().undo();
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, [70.0, 170.0, 214.0, 226.0]);
        h.state_mut().redo();
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, expected);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_import_cancel_clear_and_errors_preserve_saved_signatures() {
    let path = signature_file("cancel");
    let mut h = harness();
    let original = SavedSig::Typed("Ada Lovelace".into());
    h.state_mut().signature = Some(original.clone());
    h.state_mut().execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Image").click();
    h.run_steps(2);
    h.state_mut().pick_override = Some(Vec::new());
    h.get_by_label("Browse…").click();
    h.run_steps(3);
    assert!(h.state().signature_draft.image.is_none());
    browse_image(&mut h, &path);
    let loaded = h.state().signature_draft.image.clone();
    std::fs::write(&path, b"broken PNG").unwrap();
    browse_image(&mut h, &path);
    assert_eq!(h.state().signature_draft.image, loaded, "an invalid replacement keeps the previous draft");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Couldn't import"));
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(h.state().signature_draft.saved().is_none());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(original));
    assert!(items(&h).is_empty());
    // A picker answer from a closed dialog cannot populate a later draft.
    h.state_mut().execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Image").click();
    h.run_steps(2);
    assert_eq!(signature_file("cancel"), path, "replace the corrupt file with a valid image for the stale-answer check");
    h.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    h.get_by_label("Browse…").click();
    h.run_steps(1);
    h.state_mut().dialog = None;
    h.run_steps(3);
    assert!(h.state().signature_draft.image.is_none());
    // Malformed settings are ignored while legacy typed/drawn values still restore.
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(r#"{"signature_text":"Ada","signature_image":{"Image":"bm90IGFuIGltYWdl"},"initials":{"Image":"%%%"}}"#);
    assert_eq!(again.signature, Some(SavedSig::Typed("Ada".into())));
    assert!(again.initials.is_none());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn flatten_on_save_is_off_by_default_and_round_trips() {
    let mut app = PdfCraftApp::new();
    assert!(!app.flatten_fill_sign_on_save);
    app.set_option("flatten-fill-sign", "true").unwrap();
    assert!(app.set_option("flatten-fill-sign", "yes").is_err());
    let mut restored = PdfCraftApp::new();
    restored.restore(&app.persist());
    assert!(restored.flatten_fill_sign_on_save);
    restored.set_option("flatten-fill-sign", "false").unwrap();
    let mut again = PdfCraftApp::new();
    again.restore(&restored.persist());
    assert!(!again.flatten_fill_sign_on_save);
}

#[test]
fn saving_flattens_fill_and_sign_only_when_the_preference_is_on() {
    use pdfcraft_engine::{Edit, FillMark, NewAnnotation, Shape, Style};
    use pdfcraft_ui_egui::SaveTarget;
    use pdfcraft_ui_egui::fill_sign::TypeBox;

    let dir = std::env::temp_dir().join(format!("pdfcraft-fill-flatten-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mark = Edit::AddAnnotation(NewAnnotation {
        page: 0,
        shape: Shape::Mark { rect: [40.0, 80.0, 56.0, 96.0], mark: FillMark::Check },
        style: Style::default_for(&Shape::Mark { rect: [0.0, 0.0, 12.0, 12.0], mark: FillMark::Check }),
        contents: String::new(),
        author: "Ada".into(),
    });

    let mut app = PdfCraftApp::new();
    app.open_bytes("form.pdf", None, FIXTURE.to_vec()).unwrap();
    let id = app.views[0].id;
    app.session.apply(id, mark).unwrap();
    app.views[0].fill_text = Some(TypeBox { page: 0, at: [40.0, 200.0], text: "Typed".into(), size: 10.0, color: [0.0; 3], focus: false });
    let kept = dir.join("kept.pdf");
    app.save_override = Some(kept.to_string_lossy().into_owned());
    assert!(app.save_active(SaveTarget::InPlace));
    let mut reopened = PdfCraftApp::new();
    reopened.open_bytes("kept.pdf", None, std::fs::read(&kept).unwrap()).unwrap();
    let texts: Vec<String> =
        reopened.session.get(reopened.views[0].id).unwrap().info.annotations.iter().map(|a| a.contents.clone().unwrap_or_default()).collect();
    assert!(texts.iter().any(|t| t == "Typed"), "an open type box is saved: {texts:?}");
    assert_eq!(reopened.session.get(reopened.views[0].id).unwrap().info.annotations.len(), 2, "the check mark stays too");

    app.views[0].fill_text = Some(TypeBox { page: 0, at: [40.0, 160.0], text: "More".into(), size: 10.0, color: [0.0; 3], focus: false });
    app.flatten_fill_sign_on_save = true;
    let flat = dir.join("flat.pdf");
    app.save_override = Some(flat.to_string_lossy().into_owned());
    assert!(app.save_active(SaveTarget::InPlace));
    let mut flattened = PdfCraftApp::new();
    flattened.open_bytes("flat.pdf", None, std::fs::read(&flat).unwrap()).unwrap();
    let left = &flattened.session.get(flattened.views[0].id).unwrap().info.annotations;
    assert!(left.is_empty(), "text and the check mark are page content: {left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #299: drawn and typed signatures and initials are upright as displayed on pages with
/// `/Rotate` 90, 180 and 270, anchored at the displayed point, exactly as on an unturned page.
#[test]
fn drawn_and_typed_signatures_stay_upright_on_rotated_pages() {
    // An "L": a long stroke along the bottom and a tick up at its left end, so a turned or
    // mirrored signature shows.
    let drawn = SavedSig::Drawn(vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[0.0, 0.0], [0.0, 0.2]]]);
    let typed = SavedSig::Typed("Ada Lovelace".into());
    // (signature, initials, displayed left-centre point)
    let cases = [(&drawn, false, [40.0_f32, 60.0]), (&typed, false, [40.0, 140.0]), (&typed, true, [40.0, 230.0])];
    let ink_mask = |rotation: i64| {
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        if rotation != 0 {
            session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: rotation }).unwrap();
        }
        let page = session.get(id).unwrap().info.pages[0].clone();
        assert_eq!(i64::from(page.rotation), rotation);
        for (sig, initials, shown) in cases {
            let at = page.view_to_user(shown[0], shown[1]);
            let edit = pdfcraft_ui_egui::fill_sign::place(0, &page, [f64::from(at[0]), f64::from(at[1])], sig, initials, "Ada").unwrap();
            session.apply(id, edit).unwrap();
        }
        let annotations = &session.get(id).unwrap().info.annotations;
        assert_eq!(annotations.len(), cases.len());
        let bytes = session.get(id).unwrap().bytes.clone();
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
        let out = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(out.error.is_none(), "{:?}", out.error);
        let shown_size = if rotation % 180 == 0 { (300, 400) } else { (400, 300) };
        assert_eq!((out.width, out.height), shown_size, "rendered as displayed");
        let dark: Vec<bool> = out.rgba.as_chunks::<4>().0.iter().map(|p| p[..3].iter().all(|v| *v < 128)).collect();
        // The displayed ink box of each signature, in the band around its point.
        let boxes: Vec<[u32; 4]> = cases
            .iter()
            .map(|(_, _, shown)| {
                let (top, bottom) = ((shown[1] - 35.0) as u32, (shown[1] + 35.0) as u32);
                let mut b = [u32::MAX, u32::MAX, 0, 0];
                for y in top..bottom {
                    for x in 0..out.width {
                        if dark[(y * out.width + x) as usize] {
                            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                        }
                    }
                }
                assert!(b[0] <= b[2], "rotation {rotation}: no ink near {shown:?}");
                b
            })
            .collect();
        (dark, out.width, boxes)
    };
    let (upright, _, upright_boxes) = ink_mask(0);
    for (b, (sig, _, shown)) in upright_boxes.iter().zip(cases) {
        assert!(b[2] - b[0] > b[3] - b[1], "{sig:?} reads across: {b:?}");
        assert!(b[0].abs_diff(shown[0] as u32) <= 2, "{sig:?} starts at the point: {b:?}");
        assert!(b[1] < shown[1] as u32 && b[3] > shown[1] as u32, "{sig:?} is centred on the point: {b:?}");
    }
    // The drawn "L" has its tick at the top left, not mirrored or upside down.
    let at = |x: u32, y: u32| upright[(y * 300 + x) as usize];
    let b = upright_boxes[0];
    assert!(at(b[0] + 1, b[1] + 2) && !at(b[2] - 2, b[1] + 2) && at(b[2] - 2, b[3] - 1), "the drawn L is upright: {b:?}");
    for rotation in [90, 180, 270] {
        let (dark, width, boxes) = ink_mask(rotation);
        for (got, want) in boxes.iter().zip(&upright_boxes) {
            assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 1), "rotation {rotation}: displayed box {got:?}, upright {want:?}");
        }
        // Compare where both pages are shown (the turned ones are 400 x 300).
        let (mut differ, mut ink) = (0, 0);
        for y in 0..300 {
            for x in 0..300 {
                let (want, got) = (upright[(y * 300 + x) as usize], dark[(y * width + x) as usize]);
                ink += usize::from(want);
                differ += usize::from(want != got);
            }
        }
        assert!(differ * 10 < ink, "rotation {rotation}: {differ} of {ink} ink pixels differ from the unturned page");
    }
}

/// Press, move in steps and release, from one user-space point to another.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: (f32, f32), to: (f32, f32)) {
    let (a, b) = (at(h, from.0, from.1), at(h, to.0, to.1));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(a + (b - a) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
}

fn rects(h: &Harness<'static, PdfCraftApp>) -> Vec<[f32; 4]> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().map(|a| a.rect).collect()
}

/// A placed signature moves while a Fill & Sign tool is still active (as in Acrobat), instead of
/// the drag doing nothing and a click stamping another mark on top of it. Elsewhere on the page
/// the tool still places its mark.
#[test]
fn fill_and_sign_tools_pick_up_placed_marks_to_move_and_resize_them() {
    for tool in ["sign.fill.signature", "sign.fill.initials", "sign.fill.text", "sign.fill.check", "sign.fill.date"] {
        let mut h = harness();
        h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
        h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
        h.state_mut().execute("sign.fill.signature");
        click(&mut h, 40.0, 250.0);
        let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
        assert!(h.state_mut().execute(tool));
        h.run_steps(2);
        let mid = ((placed[0] + placed[2]) / 2.0, (placed[1] + placed[3]) / 2.0);
        h.hover_at(at(&h, mid.0, mid.1));
        h.run_steps(2);
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move, "{tool}: a placed mark can be picked up");

        drag(&mut h, mid, (mid.0 + 40.0, mid.1 - 60.0));
        assert!(matches!(h.state().quick_tool, QuickTool::Fill(_)), "{tool}: the tool stays");
        let moved = [placed[0] + 40.0, placed[1] - 60.0, placed[2] + 40.0, placed[3] - 60.0];
        let [now] = rects(&h)[..] else { panic!("{tool}: still one mark: {:?}", rects(&h)) };
        assert_rect(now, moved);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Move comment"), "{tool}");
        assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "{tool}");

        // The selected signature's handle resizes it.
        let corner = at(&h, moved[2], moved[1]);
        h.hover_at(corner);
        h.run_steps(2);
        h.drag_at(corner);
        h.run_steps(1);
        h.hover_at(corner + egui::vec2(30.0, 10.0));
        h.run_steps(2);
        h.drop_at(corner + egui::vec2(30.0, 10.0));
        h.run_steps(3);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"), "{tool}");
        assert_eq!(rects(&h).len(), 1, "{tool}");

        // A click on it selects it rather than placing another mark on top.
        h.state_mut().views[0].comments.selected = None;
        let r = rects(&h)[0];
        click(&mut h, (r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        assert_eq!(rects(&h).len(), 1, "{tool}: no mark stamped on the signature");
        assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "{tool}");
        assert!(h.state().views[0].fill_text.is_none(), "{tool}: no text box opened on the signature");
    }
    // A drawn signature in a saved and reopened file (read by the viewer, not the editor).
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Drawn(vec![vec![[0.05, 0.1], [0.3, 0.2], [0.55, 0.05], [0.95, 0.15]]]));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let saved = h.state().session.save_bytes(h.state().views[0].id).unwrap();
    let mut h = harness_bytes(&saved);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    h.state_mut().execute("sign.fill.check");
    let mid = ((placed[0] + placed[2]) / 2.0, (placed[1] + placed[3]) / 2.0);
    drag(&mut h, mid, (mid.0 + 40.0, mid.1 - 60.0));
    assert_rect(rects(&h)[0], [placed[0] + 40.0, placed[1] - 60.0, placed[2] + 40.0, placed[3] - 60.0]);

    // Away from Fill & Sign marks, the tool still places (checks stay repeatable), also on other
    // comments.
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let id = h.state().views[0].id;
    let square = pdfcraft_engine::Shape::Rectangle { rect: [100.0, 80.0, 200.0, 140.0] };
    let style = pdfcraft_engine::Style::default_for(&square);
    let new = pdfcraft_engine::NewAnnotation { page: 0, shape: square, style, contents: String::new(), author: "Ada".into() };
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::AddAnnotation(new)).unwrap();
    h.run_steps(2);
    h.state_mut().execute("sign.fill.check");
    click(&mut h, 60.0, 100.0);
    click(&mut h, 150.0, 110.0);
    assert_eq!(items(&h).len(), 4, "{:?}", items(&h));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));
}

/// A drawn signature resizes from its handles like typed and image ones: the corners keep its
/// shape, the strokes scale with it, and the size survives undo, redo and saving.
#[test]
fn drawn_signatures_resize_from_their_corners() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Drawn(vec![vec![[0.05, 0.1], [0.3, 0.2], [0.55, 0.05], [0.95, 0.15]]]));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the placed signature is selected");

    // The top-right corner handle, dragged out and a little up.
    let corner = at(&h, placed[2], placed[3]);
    h.hover_at(corner);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNeSw);
    h.drag_at(corner);
    h.run_steps(1);
    for k in 1..=10 {
        h.hover_at(corner + egui::vec2(6.0 * k as f32, -(k as f32)));
        h.run_steps(1);
    }
    h.drop_at(corner + egui::vec2(60.0, -10.0));
    h.run_steps(4);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Resize comment"));
    let [now] = rects(&h)[..] else { panic!("still one signature: {:?}", rects(&h)) };
    let ratio = |r: [f32; 4]| (r[2] - r[0]) / (r[3] - r[1]);
    assert!(now[2] - now[0] > placed[2] - placed[0] + 20.0, "wider: {placed:?} -> {now:?}");
    assert!((ratio(now) - ratio(placed)).abs() < 0.02 * ratio(placed), "same shape: {placed:?} -> {now:?}");
    assert!((now[0] - placed[0]).abs() < 0.01 && (now[1] - placed[1]).abs() < 0.01, "the opposite corner stays put");

    let id = h.state().views[0].id;
    h.state_mut().session.undo(id).unwrap();
    assert_rect(rects(&h)[0], placed);
    h.state_mut().session.redo(id).unwrap();
    assert_rect(rects(&h)[0], now);
    let saved = h.state().session.save_bytes(id).unwrap();
    let h = harness_bytes(&saved);
    assert_rect(rects(&h)[0], now);
}

/// Whether black ink is drawn within 3 pixels of `p`.
fn ink_near(pixels: &image::RgbaImage, p: Pos2) -> bool {
    (-3..=3).any(|dy| (-3..=3).any(|dx| pixels.get_pixel((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32).0[..3].iter().all(|v| *v < 60)))
}

/// A Fill & Sign line, like every stamp, is stretched with its frame while a handle is dragged:
/// it grows and shrinks with the pointer instead of keeping its old length until the release.
#[test]
fn lines_grow_and_shrink_with_their_handles_before_release() {
    let _gpu = gpu();
    for (document_rotation, view_rotation) in [(0, 0), (90, 0), (0, 90), (90, 270)] {
        let rot = (document_rotation, view_rotation);
        // A line runs across the page as /Rotate displays it (#3781): along user y on a quarter turn.
        // `d` is the distance along the line from the click at (100, 250).
        let along =
            |h: &Harness<'static, PdfCraftApp>, d: f32| if document_rotation == 90 { at(h, 100.0, 250.0 + d) } else { at(h, 100.0 + d, 250.0) };
        let line = |from: f32, to: f32| {
            if document_rotation == 90 { [98.0, 250.0 + from, 102.0, 250.0 + to] } else { [100.0 + from, 248.0, 100.0 + to, 252.0] }
        };
        let mut h = harness();
        if document_rotation != 0 {
            h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: document_rotation });
        }
        while h.state().views[0].rotation != view_rotation {
            h.state_mut().views[0].rotate_view(true);
        }
        h.run_steps(3);
        assert!(h.state_mut().execute("sign.fill.line"));
        click(&mut h, 100.0, 250.0);
        assert_rect(rects(&h)[0], line(-18.0, 18.0));
        // In line mode a click on the line picks it up, with its handles.
        click(&mut h, 100.0, 250.0);
        assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "{rot:?}");
        let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();

        // The handle at the line's end, dragged out to 60 from the middle.
        h.drag_at(along(&h, 18.0));
        h.run_steps(1);
        h.hover_at(along(&h, 60.0));
        // Wait only for the one-time layers. Later pointer frames are immediate.
        let mut grown = h.render().unwrap();
        for _ in 0..50 {
            if ink_near(&grown, along(&h, 50.0)) {
                break;
            }
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
            grown = h.render().unwrap();
        }
        assert!(ink_near(&grown, along(&h, 50.0)), "{rot:?}: the line grows with the handle before release");
        assert!(ink_near(&grown, along(&h, -10.0)), "{rot:?}: from where it starts");

        // Back in, shorter than it was, still without letting go.
        h.hover_at(along(&h, 0.0));
        h.run_steps(2);
        let shrunk = h.render().unwrap();
        assert!(ink_near(&shrunk, along(&h, -10.0)), "{rot:?}: the line still starts where it did");
        for d in [10.0, 50.0] {
            assert!(!ink_near(&shrunk, along(&h, d)), "{rot:?}: the line shrinks with the handle before release (ink at {d})");
        }
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!((doc.edit_generation(), doc.can_undo()), (generation, Some("Add line")), "{rot:?}: pointer frames never edit the document");
        if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            grown.save(format!("{dir}/line-live-grow-{document_rotation}-{view_rotation}.png")).unwrap();
            shrunk.save(format!("{dir}/line-live-shrink-{document_rotation}-{view_rotation}.png")).unwrap();
        }
        h.drop_at(along(&h, 0.0));
        h.run_steps(3);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Resize comment"), "{rot:?}");
        let [now] = rects(&h)[..] else { panic!("{rot:?}: still one line: {:?}", rects(&h)) };
        assert!(now.iter().zip(line(-18.0, 0.0)).all(|(a, b)| (a - b).abs() < 0.01), "{rot:?}: {now:?}");
    }
}

/// A signature placed over a form field is picked up by the Select tool: the field under it
/// doesn't take the press. Away from the signature, a click still fills the field.
#[test]
fn select_tool_picks_up_a_signature_over_a_form_field() {
    let mut h = harness_bytes(include_bytes!("../data/form.pdf"));
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().execute("sign.fill.signature");
    click(&mut h, 150.0, 270.0);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    // Onto the `city` text field (user space y 180..200).
    let mid_y = (placed[1] + placed[3]) / 2.0;
    let id = h.state().views[0].id;
    let index = h.state().session.get(id).unwrap().info.annotations[0].index;
    let dy = (190.0 - mid_y) as f64;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::MoveAnnotation { page: 0, index, dx: 0.0, dy }).unwrap();
    h.state_mut().views[0].comments.selected = None;
    h.state_mut().quick_tool = QuickTool::Select;
    h.run_steps(3);
    let [over] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    let mid = ((over[0] + over[2]) / 2.0, (over[1] + over[3]) / 2.0);

    h.hover_at(at(&h, mid.0, mid.1));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move);
    // A hand's drag: a few points per frame, so it starts while still over the field.
    let (a, b) = (at(&h, mid.0, mid.1), at(&h, mid.0, mid.1 + 80.0));
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=40 {
        h.hover_at(a + (b - a) * (k as f32 / 40.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_rect(rects(&h)[0], [over[0], over[1] + 80.0, over[2], over[3] + 80.0]);
    assert!(h.state().views[0].forms.focus.is_none(), "the field under the signature isn't focused");

    // Clear of the signature, the field still takes the click.
    click(&mut h, 270.0, 190.0);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
}

/// A synthetic phone photo of a signature on paper: a dark table along the left edge, a sheet
/// with a lighting gradient and a soft shadow, and a ballpoint-blue signature in two words.
fn signature_photo(test: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pdfcraft-signature-photo-{test}-{}.png", std::process::id()));
    let image = image::RgbImage::from_fn(800, 600, |x, y| {
        if x < 60 {
            return image::Rgb([70, 55, 45]);
        }
        let light = 235.0 - x as f32 * 0.04 - if (500..700).contains(&x) && y > 380 { 45.0 } else { 0.0 };
        let (fx, fy) = (x as f32, y as f32);
        let wave = |x0: f32, x1: f32| (x0..x1).contains(&fx) && (fy - (300.0 + 25.0 * ((fx - x0) / 18.0).sin())).abs() < 3.0;
        if wave(180.0, 380.0) || wave(420.0, 640.0) {
            image::Rgb([30, 40, 110])
        } else {
            let v = light.clamp(0.0, 255.0) as u8;
            image::Rgb([v, v, v.saturating_sub(6)])
        }
    });
    image.save(&path).unwrap();
    path
}

fn choose_photo(h: &mut Harness<'static, PdfCraftApp>, path: &std::path::Path) {
    h.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    h.get_by_label("Choose photo…").click();
    h.run_steps(3);
    settle_scan(h);
}

/// Scans run on a worker thread: step frames until the answer has been picked up.
fn settle_scan(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..1000 {
        if !h.state().signature_draft.scan.as_ref().is_some_and(|s| s.scanning()) {
            h.run_steps(1);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    panic!("the signature scan did not finish");
}

/// A transparent signature in the ink's colour: no paper, table or shadow is left.
fn assert_clean_signature(image: &pdfcraft_engine::SignatureImage) {
    let [w, h] = image.size();
    assert!(w > 200 && w > h * 2, "both words are framed and trimmed: {w}×{h}");
    let rgba = image.rgba();
    let opaque: Vec<&[u8; 4]> = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 128).collect();
    assert!(!opaque.is_empty());
    assert!(opaque.iter().all(|p| p[2] > p[0]), "strokes keep their blue ink");
    assert!(rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 0).count() > rgba.len() / 8, "the paper is transparent");
}

#[test]
fn scanned_signatures_are_framed_cleaned_adjusted_placed_and_remembered() {
    let _gpu = gpu();
    let path = signature_photo("scan");
    let mut h = harness();
    h.state_mut().execute("sign.fill.signature");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    assert!(h.state().signature_draft.scan_mode);
    assert!(h.state().signature_draft.saved().is_none(), "Apply needs a photo");
    choose_photo(&mut h, &path);
    let scan = h.state().signature_draft.scan.clone().expect("the photo is loaded");
    assert_eq!(scan.photo.size(), [800, 600]);
    let frame = scan.frame;
    assert!(frame.x > 60 && frame.x < 190 && frame.x + frame.width > 630, "the frame holds both words, not the table: {frame:?}");
    assert_clean_signature(h.state().signature_draft.image.as_ref().expect("a cleaned signature"));
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/scan-signature-dialog.png")).unwrap();
    }

    // Drawing a frame around the first word only leaves the second out.
    let area = h.get_by_label("Photo").rect();
    let to_screen = |x: f32, y: f32| {
        let fit = area.shrink(8.0);
        let scale = (fit.width() / 800.0).min(fit.height() / 600.0);
        let origin = fit.center() - egui::vec2(400.0, 300.0) * scale;
        origin + egui::vec2(x, y) * scale
    };
    let (a, b) = (to_screen(110.0, 190.0), to_screen(400.0, 370.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(a + (b - a) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
    settle_scan(&mut h);
    let one_word = h.state().signature_draft.scan.as_ref().unwrap().frame;
    assert!(one_word.x + one_word.width < 420, "{one_word:?}");
    let [w, _] = h.state().signature_draft.image.as_ref().unwrap().size();
    assert!(w < 300, "only the first word is left: {w}");
    assert!(h.state().signature_draft.saved().is_some(), "the full result is ready after the drag");

    // Turning the photo turns the frame with it; turning back restores both.
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.scan.as_ref().unwrap().photo.size(), [600, 800]);
    h.get_by_label("Rotate counterclockwise").click();
    h.run_steps(2);
    let scan = h.state().signature_draft.scan.clone().unwrap();
    assert_eq!((scan.photo.size(), scan.frame), ([800, 600], one_word));

    // A photo with nothing in the frame says so instead of offering an empty signature.
    {
        let d = &mut h.state_mut().signature_draft;
        let scan = d.scan.as_mut().unwrap();
        scan.frame = pdfcraft_engine::PhotoRect { x: 650, y: 40, width: 120, height: 100 };
        scan.rescan(&mut d.image, true);
    }
    h.run_steps(2);
    assert!(h.state().signature_draft.saved().is_none());
    assert!(h.state().signature_draft.scan.as_ref().unwrap().problem.as_ref().unwrap().contains("No signature was found"));
    h.get_by_label("No signature was found in the frame. Move the frame over the signature or raise the ink sensitivity.");
    {
        let d = &mut h.state_mut().signature_draft;
        let scan = d.scan.as_mut().unwrap();
        scan.frame = frame;
        scan.rescan(&mut d.image, true);
    }
    h.run_steps(2);

    h.get_by_label("Apply").click();
    h.run_steps(3);
    let Some(SavedSig::Image(saved)) = h.state().signature.clone() else { panic!("the scan is saved as an image signature") };
    assert_clean_signature(&saved);
    click(&mut h, 40.0, 250.0);
    assert_eq!(items(&h).len(), 1);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].subtype, "Stamp");

    // It is remembered like any image signature, without the photo or its path.
    let settings = h.state().persist();
    assert!(!settings.contains(&path.to_string_lossy().to_string()));
    assert!(settings.len() < 200_000, "only the trimmed signature is kept: {} bytes", settings.len());
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&settings);
    assert_eq!(again.signature, h.state().signature);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn scan_cancel_errors_and_stale_answers_preserve_saved_signatures() {
    let path = signature_photo("scan-cancel");
    let mut h = harness();
    let original = SavedSig::Typed("Ada Lovelace".into());
    h.state_mut().signature = Some(original.clone());
    h.state_mut().execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    h.state_mut().pick_override = Some(Vec::new());
    h.get_by_label("Choose photo…").click();
    h.run_steps(3);
    assert!(h.state().signature_draft.scan.is_none(), "a cancelled pick loads nothing");
    choose_photo(&mut h, &path);
    let loaded = h.state().signature_draft.clone();
    assert!(loaded.image.is_some());
    let broken = std::env::temp_dir().join(format!("pdfcraft-signature-photo-broken-{}.jpg", std::process::id()));
    std::fs::write(&broken, b"\xFF\xD8\xFF not a JPEG").unwrap();
    choose_photo(&mut h, &broken);
    assert_eq!(h.state().signature_draft, loaded, "an invalid photo keeps the previous one");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Couldn't use the photo"));
    // A photo picked for the Scan tab does not land in the Image tab.
    h.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    h.get_by_label("Choose photo…").click();
    h.run_steps(1);
    h.state_mut().signature_draft.scan = None;
    h.state_mut().signature_draft.image = None;
    h.state_mut().signature_draft.scan_mode = false;
    h.state_mut().signature_draft.image_mode = true;
    h.run_steps(3);
    assert!(h.state().signature_draft.image.is_none() && h.state().signature_draft.scan.is_none());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(original));
    assert!(items(&h).is_empty());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(broken).unwrap();
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(std::path::PathBuf, Vec<u8>);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.1.clone())
    }
}

#[test]
fn photos_and_images_dropped_on_the_signature_pad_are_imported_not_opened() {
    let photo = signature_photo("drop");
    let mut h = harness();
    let tabs = h.state().views.len();
    h.state_mut().execute("sign.fill.signature");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    // From a file manager (an absolute path) and from a browser-like source (bytes only).
    for file in [Dropped(photo.clone(), Vec::new()), Dropped("photo.png".into(), std::fs::read(&photo).unwrap())] {
        h.state_mut().signature_draft.scan = None;
        h.input_mut().dropped_files.push(std::sync::Arc::new(file));
        h.run_steps(3);
        settle_scan(&mut h);
        assert!(h.state().signature_draft.scan.is_some(), "the photo is scanned");
        assert!(h.state().signature_draft.image.is_some());
    }
    h.get_by_label("Image").click();
    h.run_steps(2);
    h.state_mut().signature_draft.image = None;
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped("photo.png".into(), std::fs::read(&photo).unwrap())));
    h.run_steps(3);
    assert!(h.state().signature_draft.image.is_some(), "the Image tab takes the picture as it is");
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped("broken.png".into(), b"broken".to_vec())));
    h.run_steps(3);
    assert!(h.state().signature_draft.image.is_some(), "an invalid file keeps the previous draft");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Couldn't import"));
    assert_eq!(h.state().views.len(), tabs, "nothing opened behind the dialog");
    std::fs::remove_file(photo).unwrap();
}

/// Where a mode bar tab is, by its label.
fn mode_tab(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Rect {
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, label).map(|n| n.rect()).find(|r| r.bottom() < 120.0).expect("a mode tab")
}

/// Scroll the page until `what` (on screen) lies across the mode bar, then the cursor over each
/// tab it lies under: with the All tools panel closed, every tab: All tools, Edit, Convert and E-Sign.
fn mode_tab_cursors(
    h: &mut Harness<'static, PdfCraftApp>,
    what: impl Fn(&Harness<'static, PdfCraftApp>) -> egui::Rect,
) -> [(&'static str, egui::CursorIcon); 4] {
    let page = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].page_screen_rect(0).expect("on screen");
    let y = what(h).center().y - page(h).top();
    let tabs = mode_tab(h, "Convert").center().y;
    h.state_mut().views[0].goto = Some((0, 0.0));
    h.run_steps(4);
    let top = page(h).top();
    h.state_mut().views[0].goto = Some((0, (top - (tabs - y)) / page(h).height()));
    h.run_steps(4);
    let under = what(h);
    ["All tools", "Edit", "Convert", "E-Sign"].map(|label| {
        let at = mode_tab(h, label).center();
        assert!(under.contains(at), "{under:?} lies under {label} at {at:?}");
        h.hover_at(at);
        h.run_steps(2);
        (label, h.output().platform_output.cursor_icon)
    })
}

const HAND_ON_EVERY_TAB: [(&str, egui::CursorIcon); 4] = [
    ("All tools", egui::CursorIcon::PointingHand),
    ("Edit", egui::CursorIcon::PointingHand),
    ("Convert", egui::CursorIcon::PointingHand),
    ("E-Sign", egui::CursorIcon::PointingHand),
];

/// The Fill & Sign text box scrolled up under the mode bar leaves its tabs the hand (#2755): it
/// floated over them, gave them its I-beam and took their clicks.
#[test]
fn mode_tabs_keep_the_hand_over_a_type_box_scrolled_under_them() {
    let mut h = harness();
    assert!(h.state_mut().execute("sign.fill.text"));
    h.state_mut().set_option("left", "closed").unwrap();
    // The page across the window, so the box reaches under the tabs.
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Width;
    h.run_steps(4);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Text));
    click(&mut h, 10.0, 360.0);
    h.event(egui::Event::Text("Ada Lovelace, Countess of Lovelace".into()));
    h.run_steps(3);
    assert_eq!(mode_tab_cursors(&mut h, type_box_rect), HAND_ON_EVERY_TAB);
    let convert = mode_tab(&h, "Convert").center();
    h.hover_at(convert);
    h.run_steps(1);
    h.drag_at(convert);
    h.run_steps(1);
    h.drop_at(convert);
    h.run_steps(3);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Convert, "the tab takes the click, not the text box");
}
