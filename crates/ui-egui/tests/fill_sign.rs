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
    harness_bytes(FIXTURE)
}

fn harness_bytes(fixture: &[u8]) -> Harness<'static, PdfCraftApp> {
    let fixture = fixture.to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
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

fn signature_file(test: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pdfcraft-signature-{test}-{}.png", std::process::id()));
    let image = image::RgbaImage::from_fn(120, 40, |x, y| {
        if (10..110).contains(&x) && (15..25).contains(&y) || (10..20).contains(&x) && (5..15).contains(&y) {
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
    let path = signature_file("pointer");
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
    // Exercise document rotation and view rotation separately and together.
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
            let ink = at(&h, 90.0, y);
            let margin = at(&h, 42.0, y);
            let upper_stroke = at(&h, 52.0, y + 8.0);
            let lower_margin = at(&h, 52.0, y - 8.0);
            let pixels = h.render().unwrap();
            assert!(pixels.get_pixel(ink.x as u32, ink.y as u32).0[..3].iter().all(|v| *v < 60), "ink follows the pointer at the placed size");
            assert!(pixels.get_pixel(margin.x as u32, margin.y as u32).0[..3].iter().all(|v| *v > 240), "alpha exposes the page");
            assert!(
                pixels.get_pixel(upper_stroke.x as u32, upper_stroke.y as u32).0[..3].iter().all(|v| *v < 60),
                "the asymmetric image rotates with the page"
            );
            assert!(pixels.get_pixel(lower_margin.x as u32, lower_margin.y as u32).0[..3].iter().all(|v| *v > 240), "the image isn't flipped");
            if y == 150.0 {
                let old = at(&h, 90.0, 250.0);
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
        click(&mut h, 40.0, 250.0);
        rendered_ink(&mut h, 90.0, 250.0);
        // Future placements may use a different signature: drag the PDF's embedded image.
        h.state_mut().signature = None;
        h.drag_at(at(&h, 136.0, 234.0));
        h.run_steps(1);
        let end = at(&h, 184.0, 228.0);
        h.hover_at(end);
        rendered_ink(&mut h, 160.0, 242.0);
        let pixels = h.render().unwrap();
        assert!(pixel_at(&h, &pixels, 58.0, 254.0)[..3].iter().all(|v| *v < 60), "asymmetric ink rotates during resize");
        assert!(pixel_at(&h, &pixels, 58.0, 230.0)[..3].iter().all(|v| *v > 240), "alpha isn't flipped");
        h.drop_at(end);
        h.run_steps(3);
        h.drag_at(at(&h, 112.0, 242.0));
        h.run_steps(1);
        h.hover_at(at(&h, 132.0, 142.0));
        h.run_steps(2);
        let pixels = h.render().unwrap();
        assert!(pixel_at(&h, &pixels, 132.0, 142.0)[..3].iter().all(|v| *v < 60), "image moves before release");
        assert!(pixel_at(&h, &pixels, 160.0, 242.0)[..3].iter().all(|v| *v > 240), "old ink is removed during movement");
        assert!(!has_blue_near(&pixels, at(&h, 204.0, 166.0)));
        h.drop_at(at(&h, 132.0, 142.0));
        h.run_steps(2);
        assert!(has_blue_near(&h.render().unwrap(), at(&h, 204.0, 166.0)));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_corners_restore_original_aspect_after_edge_resize_and_reopen() {
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
    again.restore(r#"{"signature_text":"Ada","signature_image":{"Image":"bm90IGFuIGltYWdl"},"initials":{"Image":"%%%"}}"#);
    assert_eq!(again.signature, Some(SavedSig::Typed("Ada".into())));
    assert!(again.initials.is_none());
    std::fs::remove_file(path).unwrap();
}
