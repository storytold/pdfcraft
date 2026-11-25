//! Preferences ▸ Search ▸ Ignore Asian character width decides whether find matches halfwidth and
//! fullwidth forms at their ordinary width: in the find bar, in Search & Redact, and across a
//! restart.

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::import_json;

/// One page in a Helvetica whose `/ToUnicode` maps its codes to halfwidth and fullwidth forms:
/// `ＡＢＣ１２３` on one line, and below it `ｶﾞｲﾄﾞ` with each halfwidth sound mark a glyph of its own.
fn width_pdf() -> Vec<u8> {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 10 beginbfchar \
                <41> <FF21> <42> <FF22> <43> <FF23> <31> <FF11> <32> <FF12> <33> <FF13> \
                <44> <FF76> <45> <FF9E> <46> <FF72> <47> <FF84> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let body = "BT /F1 14 Tf 90 700 Td (ABC123) Tj 0 -30 Td (DEFGE) Tj ET\n";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let ctx = egui::Context::default();
        let mut app = crate::control::new_app();
        app.set_option(&ctx, "language", "en").unwrap();
        app.open_bytes(&ctx, "width.pdf", None, width_pdf()).expect("opens");
        app.set_option(&ctx, "find", "abc123").unwrap();
        app
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// Click Preferences' row `label` (one scrolling page: brought into view first).
fn click_row(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by(|n| !n.is_hidden() && n.role() != egui::accesskit::Role::Label && n.label().as_deref() == Some(label)).scroll_to_me();
    h.run_steps(3);
    h.get_by(|n| !n.is_hidden() && n.role() != egui::accesskit::Role::Label && n.label().as_deref() == Some(label)).click();
    h.run_steps(4);
}

fn matches(h: &Harness<'static, PdfCraftApp>) -> usize {
    h.state().views[0].find.as_ref().map_or(0, |f| f.matches.len())
}

/// The preference is on by default and the find bar finds the fullwidth letters from ASCII;
/// unticking it in Preferences searches again and finds nothing, and ticking it finds them again.
#[test]
fn unticking_ignore_asian_character_width_makes_the_find_bar_match_width() {
    let mut h = harness();
    assert!(h.state().ignore_asian_width, "on by default");
    assert_eq!(matches(&h), 1, "ABC123 finds ＡＢＣ１２３");

    h.state_mut().open_preferences();
    h.run_steps(4);
    click_row(&mut h, "Ignore Asian character width");
    assert!(!h.state().ignore_asian_width);
    assert_eq!(matches(&h), 0, "with width significant, ASCII is not fullwidth");

    click_row(&mut h, "Ignore Asian character width");
    assert!(h.state().ignore_asian_width);
    assert_eq!(matches(&h), 1);
}

/// Search & Redact follows the preference too, and so does the control channel's `set`.
#[test]
fn search_and_redact_follows_the_preference() {
    let mut h = harness();
    let ctx = h.ctx.clone();
    h.set_option("ignore_asian_width", "false").unwrap();
    h.run_steps(2);
    assert_eq!(matches(&h), 0);
    h.state_mut().redact_search.text = "ガイド".into();
    assert_eq!(h.state_mut().redact_search(&ctx), 0, "ガ is not ｶﾞ with width significant");
    assert!(h.set_option("ignore_asian_width", "off").is_err());
    h.set_option("ignore_asian_width", "true").unwrap();
    h.run_steps(2);
    assert_eq!(matches(&h), 1);
    assert_eq!(h.state_mut().redact_search(&ctx), 1, "ガイド marks ｶﾞｲﾄﾞ");
}

/// The control channel's `set` reaches open views at once, before the window draws again (a
/// hidden window does not draw), so a view's search agrees with `ui.state`.
#[test]
fn set_reaches_open_views_without_a_draw() {
    let mut h = harness();
    assert_eq!(matches(&h), 1);
    h.set_option("ignore_asian_width", "false").unwrap();
    assert_eq!(matches(&h), 0, "the view searched again without a frame");
    h.set_option("ignore_asian_width", "true").unwrap();
    assert_eq!(matches(&h), 1);
}

/// The choice survives a restart, also from the last release's settings file; settings saved
/// before the preference existed leave it on.
#[test]
fn the_preference_is_remembered() {
    let mut app = crate::control::new_app();
    app.ignore_asian_width = false;
    let saved = app.settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&saved);
    assert!(!fresh.ignore_asian_width, "the preference is remembered");

    let mut imported = crate::control::new_app();
    imported.apply_settings(&import_json(r#"{"ignore_asian_width": false}"#).0);
    assert!(!imported.ignore_asian_width, "read from the last release's settings");

    let mut old = saved;
    old.layout.ignore_asian_width = None;
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&old);
    assert!(fresh.ignore_asian_width, "missing means the default, on");
}
