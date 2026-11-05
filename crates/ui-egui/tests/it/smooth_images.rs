//! Preferences ▸ Smooth images: the app-wide preference (Acrobat's Page Display ▸ Smooth images),
//! how it survives a restart, and how documents open with it.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A 64-pixel picture, its left half black, as PNG.
fn png() -> Vec<u8> {
    let picture = image::GrayImage::from_fn(64, 64, |x, _| image::Luma([if x < 32 { 0 } else { 255 }]));
    let mut png = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut png, image::ImageFormat::Png).unwrap();
    png.into_inner()
}

/// A one-page document that is only [`png`].
fn picture_pdf() -> Vec<u8> {
    pdfcraft_engine::Session::new().create_from_images(&[("picture.png".into(), png())]).unwrap().to_vec()
}

#[test]
fn smooth_images_preference_round_trips_and_reaches_open_documents() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("picture.pdf", None, picture_pdf()).expect("opens");
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    assert!(h.state().smooth_images, "on by default");
    assert_eq!(h.state().session.sharp_images(id), Some(false));
    h.get_by(|n| n.role() == Role::CheckBox && n.label().as_deref() == Some("Smooth images")).scroll_to_me();
    h.run_steps(4);
    h.get_by(|n| n.role() == Role::CheckBox && n.label().as_deref() == Some("Smooth images")).click();
    h.run_steps(2);
    assert!(!h.state().smooth_images);
    // The preference is app-wide: the open document is drawn unsmoothed from now on.
    assert_eq!(h.state().session.sharp_images(id), Some(true));

    // The choice survives a restart through the same path the settings file uses.
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert!(!restored.smooth_images);
    // A settings file without the field, or with a value that isn't true or false, keeps it on.
    let mut fresh = PdfCraftApp::new();
    fresh.restore(r#"{"theme": "dark"}"#);
    assert!(fresh.smooth_images);
    fresh.restore(r#"{"smooth_images": 7}"#);
    assert!(fresh.smooth_images);
}

#[test]
fn smooth_images_seed_on_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.smooth_images = false;
        app.open_bytes("picture.pdf", None, picture_pdf()).expect("opens");
        app
    });
    h.run_steps(4);
    // The document opened with the preference already applied.
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.sharp_images(id), Some(true));

    // Revisions and the signed version open through their own session calls, bypassing the
    // ordinary open: they seed the new document the same way. A second revision comes from an
    // incremental save.
    {
        let s = &mut h.state_mut().session;
        s.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
        let saved = s.save_bytes(id).unwrap();
        s.mark_saved(id, saved, None).unwrap();
    }
    h.state_mut().open_revision(1);
    let revision = h.state().views.last().unwrap().id;
    assert_ne!(revision, id);
    assert_eq!(h.state().session.sharp_images(revision), Some(true));
    let len = h.state().session.get(id).unwrap().bytes.len();
    h.state_mut().view_signed_version(len, 1);
    let signed = h.state().views.last().unwrap().id;
    assert_ne!(signed, id);
    assert_eq!(h.state().session.sharp_images(signed), Some(true));
    // Created documents open through their own session calls too: a comment summary via
    // `open_created`, an image import via `open_created_bytes`.
    h.state_mut().summarize_comments();
    let summary = h.state().views.last().unwrap().id;
    assert_ne!(summary, signed);
    assert_eq!(h.state().session.sharp_images(summary), Some(true));
    h.state_mut().create_from_images(vec![("scan.png".into(), png())]);
    let created = h.state().views.last().unwrap().id;
    assert_ne!(created, summary);
    assert_eq!(h.state().session.sharp_images(created), Some(true));

    // Back on through the control channel (`--smooth-images on`), for every open document.
    h.state_mut().set_option("smooth-images", "on").unwrap();
    h.run_steps(1);
    for view in &h.state().views {
        assert_eq!(h.state().session.sharp_images(view.id), Some(false));
    }
    assert!(h.state_mut().set_option("smooth-images", "maybe").is_err());
}
