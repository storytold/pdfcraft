//! Find and replace (Edit text mode): ⇧⌘H turns Edit text on and shows the find bar's Replace
//! row, Replace all rewrites every match inside its paragraph as one undo step, and outside Edit
//! text the row is not offered.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// Two 300×200 pages of Helvetica text: "cat and cat" and "a cat".
fn fixture() -> Vec<u8> {
    let stream = |content: &str| format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        stream("BT /F1 14 Tf 20 150 Td (cat and cat) Tj ET"),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        stream("BT /F1 14 Tf 20 150 Td (a cat) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
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

fn texts(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    (0..2).flat_map(|p| doc.text_blocks(p).into_iter().map(|b| b.text)).collect()
}

fn has_field(h: &Harness<'static, PdfCraftApp>, placeholder: &str) -> bool {
    h.query_all_by(|n| n.placeholder() == Some(placeholder)).next().is_some()
}

#[test]
fn replace_all_from_the_find_bar_is_one_undo_step() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cats.pdf", None, fixture()).expect("opens");
        app
    });
    h.run_steps(4);
    // ⌘F with the Select tool: the find bar, without a Replace row.
    h.key_press_modifiers(Modifiers::COMMAND, Key::F);
    h.run_steps(3);
    assert!(h.state().views[0].find.is_some());
    assert!(has_field(&h, "Find text"));
    assert!(!has_field(&h, "Replace with"), "outside Edit text the find bar doesn't replace");
    // ⇧⌘H: Edit text, and the Replace row.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::H);
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    assert!(h.state().views[0].find.as_ref().is_some_and(|f| f.replacing));
    assert!(has_field(&h, "Replace with"), "the Replace row shows in Edit text");
    h.query_by(|n| n.placeholder() == Some("Find text")).expect("find field").type_text("cat");
    // Both pages' text loads in the background before every match is listed.
    for _ in 0..400 {
        h.run_steps(1);
        if h.state().views[0].find.as_ref().is_some_and(|f| f.matches.len() == 3) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].find.as_ref().map(|f| f.matches.len()), Some(3));
    h.query_by(|n| n.placeholder() == Some("Replace with")).expect("replace field").click();
    h.run_steps(2);
    h.query_by(|n| n.placeholder() == Some("Replace with")).expect("replace field").type_text("dog");
    h.run_steps(1);
    assert_eq!(h.state().views[0].find.as_ref().map(|f| (f.query.as_str(), f.replacement.as_str())), Some(("cat", "dog")));
    // The row may settle over a few frames once the status text changes its width.
    h.run_steps(8);
    h.get_by_label("Replace all").click();
    h.run_steps(4);
    assert_eq!(texts(&h), ["dog and dog", "a dog"], "{:?}", h.state().toast);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Replace all"));
    assert_eq!(s.toast.as_ref().map(|t| t.0.as_str()), Some("Replaced 3 on 2 pages"));
    // One Undo puts every page back.
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(texts(&h), ["cat and cat", "a cat"]);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), None);
}

/// Replace changes the highlighted match only: here the second "cat" on the first page.
#[test]
fn replace_changes_the_highlighted_match() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cats.pdf", None, fixture()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.replace"));
    h.run_steps(3);
    h.query_by(|n| n.placeholder() == Some("Find text")).expect("find field").type_text("cat");
    for _ in 0..400 {
        h.run_steps(1);
        if h.state().views[0].find.as_ref().is_some_and(|f| f.matches.len() == 3) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.state_mut().views[0].find.as_mut().unwrap().replacement = "dog".into();
    h.state_mut().views[0].go_to_match(1);
    h.run_steps(8);
    h.get_by_label("Replace").click();
    h.run_steps(4);
    assert_eq!(texts(&h), ["cat and dog", "a cat"]);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Edit text"));
}
