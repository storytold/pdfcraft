//! The smallest window (#1913): at `MIN_WINDOW_SIZE` the mode bar folds the modes into a menu
//! and keeps the command search reachable, instead of hiding them under the search field.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{MIN_WINDOW_SIZE, PdfCraftApp};

fn harness(size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(8);
    h
}

#[test]
fn at_the_minimum_width_the_modes_fold_into_a_menu_and_search_stays() {
    let mut h = harness(egui::vec2(MIN_WINDOW_SIZE[0], MIN_WINDOW_SIZE[1]));
    assert!(h.query_by_role_and_label(Role::Button, "Convert").is_none(), "no room for the tabs");
    assert!(h.query_by_role_and_label(Role::Button, "Find tools and commands").is_some(), "command search is reachable");
    h.get_by_role_and_label(Role::Button, "All tools").click();
    h.run_steps(4);
    h.get_by_role_and_label(Role::Button, "E-Sign").click();
    h.run_steps(4);
    assert!(h.query_by_role_and_label(Role::Button, "E-Sign").is_some(), "the menu button is named after the mode");
    assert!(h.query_by_role_and_label(Role::Button, "Menu").is_some(), "main menu reachable");
}

#[test]
fn a_wide_window_keeps_the_mode_tabs() {
    let h = harness(egui::vec2(1400.0, 900.0));
    for mode in ["All tools", "Edit", "Convert", "E-Sign"] {
        assert!(h.query_by_role_and_label(Role::Button, mode).is_some(), "{mode} tab");
    }
}

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

fn protected(user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    protected_with(pdfcraft_cos::Algorithm::Aes256, user, owner, permissions)
}

fn protected_with(algorithm: pdfcraft_cos::Algorithm, user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// A narrow window: the stacked notice bar keeps both buttons inside the window and apart.
#[test]
fn the_stacked_notice_keeps_its_buttons_in_view() {
    let mut h = Harness::builder().with_size(egui::vec2(MIN_WINDOW_SIZE[0], MIN_WINDOW_SIZE[1])).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap();
        app
    });
    h.run_steps(8);
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(MIN_WINDOW_SIZE[0], MIN_WINDOW_SIZE[1]));
    let msg = h.get_by_label_contains("This document is secured").rect();
    let (unlock, settings) = (h.get_by_label("Unlock…").rect(), h.get_by_label("Security settings").rect());
    for (name, r) in [("unlock", unlock), ("settings", settings)] {
        assert!(window.contains_rect(r), "{name} stays inside the window: {r:?}");
        assert!(r.left() >= msg.left() - 30.0, "{name} is not cut off at the bar's left edge: {r:?} vs {msg:?}");
        assert!(r.top() >= msg.bottom() - 0.5 || r.bottom() <= msg.top() + 0.5, "{name} does not cover the message");
    }
    assert!(!unlock.intersects(settings), "the buttons do not overlap: {unlock:?} {settings:?}");
}
