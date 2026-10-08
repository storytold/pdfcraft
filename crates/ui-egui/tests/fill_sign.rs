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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app.set_option("author", "Ada").unwrap();
        app
    });
    h.run_steps(5);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (400.0 - y) / 400.0 * r.height())
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

#[test]
fn text_marks_and_date() {
    let mut h = harness();
    assert!(h.state_mut().execute("sign.fill.check"));
    assert!(matches!(h.state().quick_tool, QuickTool::Fill(_)));
    click(&mut h, 50.0, 350.0);
    h.state_mut().execute("sign.fill.date");
    click(&mut h, 50.0, 300.0);
    h.state_mut().execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let v = items(&h);
    assert_eq!(v.len(), 3, "{v:?}");
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref() == Some("Ada Lovelace")));
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref().is_some_and(|c| c.matches('/').count() == 2)), "a date: {v:?}");
    assert!(v.iter().any(|(t, _)| t == "Stamp"));
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
    // The signature is remembered (persisted with the app's settings).
    let saved = h.state().persist();
    let mut again = PdfCraftApp::new();
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
    again.restore(&h.state().persist());
    assert_eq!(again.signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(again.initials, Some(SavedSig::Typed("GH".into())));
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
    assert_eq!(h.state().signature_draft.saved(), drawn);
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(h.state().signature_draft.strokes.is_empty());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(drawn.clone()));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));

    assert!(h.state_mut().execute("sign.fill.signature.change"));
    h.run_steps(2);
    assert_eq!(h.state().signature_draft.saved(), drawn);
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
    let pdfcraft_engine::Edit::AddAnnotation(a) = pdfcraft_ui_egui::fill_sign::place(0, [40.0, 200.0], &sig, false, "").unwrap() else {
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
