//! The page cursor stays off while a dialog is open, and the Space Audit tab measures when the app says it does.

use crate::support::AppHarness as _;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page file.
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
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

/// The rectangle of the open dialog, from its accessibility node.
fn dialog_rect(h: &Harness<'_, PdfCraftApp>) -> egui::Rect {
    h.get_by_role(Role::Dialog).rect()
}

/// A copy of the measured rows, read through the lock the app's job fills them under.
fn audit(app: &PdfCraftApp) -> Vec<pdfcraft_engine::optimize::SpaceUse> {
    app.space_audit.read().map(|rows| rows.clone()).unwrap_or_default()
}

/// An app with `doc.pdf` open and the Space Audit tab of Properties showing.
fn space_audit_harness() -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, fixture()).unwrap();
            app.set_option(&egui::Context::default(), "dialog", "space-audit").unwrap();
            app
        }
    })
}

#[test]
fn space_audit_tab_measures_the_file() {
    let mut h = space_audit_harness();
    crate::support::wait_until(&mut h, "the space audit", |app| !audit(app).is_empty());
    h.run_steps(2);
    let card = dialog_rect(&h);
    let total = h.get_by_role_and_label(egui::accesskit::Role::ListItem, "Total").rect();
    assert!(card.contains_rect(total), "the Total row {total:?} lies in the dialog {card:?}");
}

#[test]
fn space_audit_tab_measures_again_after_an_edit() {
    use pdfcraft_engine::optimize::SpaceCategory;
    let mut h = space_audit_harness();
    crate::support::wait_until(&mut h, "the first space audit", |app| !audit(app).is_empty());
    let bookmarks = |app: &PdfCraftApp| audit(app).iter().filter(|r| r.category == SpaceCategory::Bookmarks).map(|r| r.bytes).sum::<u64>();
    let before = audit(h.state());
    assert_eq!(bookmarks(h.state()), 0, "the fixture has no outline: {before:?}");
    // Further frames with the file unchanged keep the same measurement.
    h.run_steps(2);
    assert_eq!(audit(h.state()), before);
    // An edit while the tab is open is measured again.
    let ctx_h = h.ctx.clone();
    h.state_mut()
        .apply_edit(&ctx_h, pdfcraft_engine::Edit::AddBookmark { parent: vec![], index: 0, title: "Chapter".into(), page: 0, left: None, top: None });
    crate::support::wait_until(&mut h, "the new outline to be counted", |app| bookmarks(app) > 0);
    h.run_steps(2);
    // The table shows the new row (the rail's Bookmarks button has the same label).
    let card = dialog_rect(&h);
    assert!(h.query_all_by_label("Bookmarks").any(|n| card.contains_rect(n.rect())), "a Bookmarks row in {card:?}");
}

#[test]
fn page_hover_cursors_stay_off_while_a_dialog_is_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
            // Large enough that the drop-down sits outside the dialog.
            app.set_option(&egui::Context::default(), "zoom", "300").unwrap();
            app
        }
    });
    for _ in 0..30 {
        h.run_steps(2);
    }
    // The middle of the "country" drop-down (/Rect [20 220 200 240] on a 300 × 400 page).
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let field = page.min + egui::vec2(110.0 * k, (400.0 - 230.0) * k);
    h.hover_at(field);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "the drop-down takes clicks");
    h.set_option("dialog", "security").unwrap();
    h.run_steps(4);
    let card = dialog_rect(&h);
    assert!(!card.contains(field), "the drop-down {field:?} is beside the dialog {card:?}, under its backdrop");
    h.hover_at(field + egui::vec2(1.0, 0.0));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default, "nothing under the dialog's backdrop can be clicked");
}
