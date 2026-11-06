//! View ▸ Invert colors: the app-wide inverted view (its mode and palette), how it survives a
//! restart, and how documents open with it. What it does to pixels is tested in `pdfcraft-render` and
//! through the tools in `pdfcraft-automation`.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One valid page (proper xref): black text on white.
fn pdf() -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        {
            let body = b"BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET";
            format!("<< /Length {} >>\nstream\n{}\nendstream", body.len(), String::from_utf8_lossy(body)).into_bytes()
        },
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A small PNG for the Create ▸ Images path.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 8, 4);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[120u8; 8 * 4 * 3]).unwrap();
    }
    out
}

#[test]
fn invert_colors_preference_round_trips() {
    use pdfcraft_render::InvertPalette;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, pdf()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(!h.state().invert_colors, "off at first");
    assert!(h.state().invert_keep_images, "pictures are kept by default");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.invert_colors(id), Some(false));

    // The command toggles it, and the document follows.
    h.state_mut().execute("view.invert_colors");
    h.run_steps(1);
    assert!(h.state().invert_colors);
    assert_eq!(h.state().session.invert_colors(id), Some(true));
    h.state_mut().execute("view.invert_colors");
    h.run_steps(1);
    assert!(!h.state().invert_colors);
    assert_eq!(h.state().session.invert_colors(id), Some(false));

    // The startup option, both spellings.
    h.state_mut().set_option("invert-colors", "on").unwrap();
    assert!(h.state().invert_colors);
    h.state_mut().set_option("invert_colors", "off").unwrap();
    assert!(!h.state().invert_colors);
    assert!(h.state_mut().set_option("invert-colors", "maybe").is_err());

    // The choice, the mode and the palette survive a restart; a settings file without them keeps the defaults.
    h.state_mut().set_invert_palette(InvertPalette::SEPIA);
    h.state_mut().set_invert_keep_images(false);
    h.state_mut().set_invert_colors(true);
    let json = h.state().persist();
    let mut restored = PdfCraftApp::new();
    restored.restore(&json);
    assert!(restored.invert_colors && !restored.invert_keep_images);
    assert_eq!(restored.invert_palette, InvertPalette::SEPIA);
    let mut fresh = PdfCraftApp::new();
    fresh.restore(r#"{"theme": "dark", "invert_palette": [1, 2]}"#);
    assert!(!fresh.invert_colors && fresh.invert_keep_images);
    assert_eq!(fresh.invert_palette, InvertPalette::default(), "an unreadable palette keeps the default");
}

#[test]
fn invert_colors_seed_on_open() {
    use pdfcraft_render::InvertPalette;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.invert_palette = InvertPalette::SOFT_DARK;
        app.invert_keep_images = false;
        app.invert_colors = true;
        app.open_bytes("doc.pdf", None, pdf()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    // Every way a document opens starts with the view: what the app holds, mode and palette included.
    let holds = |h: &Harness<'_, PdfCraftApp>, id| {
        let s = &h.state().session;
        (s.invert_colors(id), s.invert_keep_images(id), s.invert_palette(id))
    };
    let expected = (Some(true), Some(false), Some(InvertPalette::SOFT_DARK));
    assert_eq!(holds(&h, id), expected);

    // Changing the preference applies to every open document.
    h.state_mut().set_invert_colors(false);
    h.run_steps(1);
    assert_eq!(h.state().session.invert_colors(id), Some(false));
    h.state_mut().set_invert_colors(true);
    h.run_steps(1);
    assert_eq!(holds(&h, id), expected);

    // Revisions, signed versions and created documents inherit it.
    {
        let s = &mut h.state_mut().session;
        s.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
        let saved = s.save_bytes(id).unwrap();
        s.mark_saved(id, saved, None).unwrap();
    }
    h.state_mut().open_revision(1);
    let revision = h.state().views.last().unwrap().id;
    assert_ne!(revision, id);
    assert_eq!(holds(&h, revision), expected);

    let len = h.state().session.get(id).unwrap().bytes.len();
    h.state_mut().view_signed_version(len, 1);
    let signed = h.state().views.last().unwrap().id;
    assert_ne!(signed, id);
    assert_eq!(holds(&h, signed), expected);

    h.state_mut().summarize_comments();
    let summary = h.state().views.last().unwrap().id;
    assert_ne!(summary, signed);
    assert_eq!(holds(&h, summary), expected);

    h.state_mut().create_from_images(vec![("scan.png".into(), png())]);
    let created = h.state().views.last().unwrap().id;
    assert_ne!(created, summary);
    assert_eq!(holds(&h, created), expected);

    // Turning it off applies to all of them.
    h.state_mut().set_invert_colors(false);
    h.run_steps(1);
    for view in &h.state().views {
        assert_eq!(h.state().session.invert_colors(view.id), Some(false));
    }
}

/// The mode and the palette reach open documents, and the Reading colours preference stays what the
/// user set: the engine draws with the inverted view instead of it while the view is on, so there is
/// nothing to restore when it is turned off.
#[test]
fn invert_mode_and_palette_reach_open_documents_and_leave_reading_colours_alone() {
    use pdfcraft_render::{InvertPalette, ReadingColors};
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, pdf()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    h.state_mut().set_reading_colors(ReadingColors::YELLOW_ON_BLACK);

    h.state_mut().set_invert_palette(InvertPalette::SOFT_DARK);
    h.state_mut().set_invert_colors(true);
    assert_eq!(h.state().session.invert_keep_images(id), Some(true));
    assert_eq!(h.state().session.invert_palette(id), Some(InvertPalette::SOFT_DARK));
    h.state_mut().set_invert_palette(InvertPalette::SEPIA);
    assert_eq!(h.state().session.invert_palette(id), Some(InvertPalette::SEPIA));
    h.state_mut().set_invert_keep_images(false);
    assert_eq!(h.state().session.invert_keep_images(id), Some(false));
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::YELLOW_ON_BLACK), "the preference is not touched");

    // A preference changed while inverted still reaches the document, and shows once the view is off.
    h.state_mut().set_reading_colors(ReadingColors::WHITE_ON_BLACK);
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
    h.state_mut().set_invert_colors(false);
    assert_eq!(h.state().session.invert_colors(id), Some(false));
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
}
