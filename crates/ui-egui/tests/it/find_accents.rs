//! The find bar ignores accents unless "Accent-sensitive" is ticked (#3460), driven like a user
//! would: type a query, open Find options, tick the box.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// Three pages of Helvetica (WinAnsi, so `\351` is é): "Café menu", "cafe table" and "CAFÉ bar".
const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 8 0 R] /Count 3 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
8 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 9 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
5 0 obj << /Length 47 >> stream
BT /F1 14 Tf 20 150 Td (Caf\\351 menu) Tj ET
endstream endobj
6 0 obj << /Length 48 >> stream
BT /F1 14 Tf 20 150 Td (cafe table) Tj ET
endstream endobj
9 0 obj << /Length 46 >> stream
BT /F1 14 Tf 20 150 Td (CAF\\311 bar) Tj ET
endstream endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(query: &'static str) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("accents.pdf", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("find", query).unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    h
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The pages the find bar has matches on, in order.
fn pages(h: &Harness<'static, PdfCraftApp>) -> Vec<usize> {
    h.state().views[0].find.as_ref().expect("the find bar is open").matches.iter().map(|(p, _)| *p).collect()
}

/// What the find bar's options are.
fn option(h: &Harness<'static, PdfCraftApp>, name: &str) -> bool {
    let find = h.state().views[0].find.as_ref().expect("the find bar is open");
    match name {
        "Accent-sensitive" => find.accent_sensitive,
        "Case-sensitive" => find.case_sensitive,
        other => panic!("not an option: {other}"),
    }
}

/// Tick `name` in the Find options menu, as a user would, and wait for the option to change.
///
/// Two things move. The button shifts as the bar's "n of m" text changes width while matches
/// arrive, and a menu is laid out over its first frames, so a click on an entry in the frame it
/// appears lands where the entry will not be, closes the menu and changes nothing. So: wait for
/// the entry's place to hold still before clicking it, and if the option did not change anyway,
/// go round again, for a bounded number of times.
fn tick(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let before = option(h, name);
    let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
    for _ in 0..20 {
        if option(h, name) != before {
            break;
        }
        if h.query_by_label(name).is_none() {
            h.get_by_label("Find options").click();
        }
        // Wait for the entry to appear and stop moving: the same place for three frames running.
        let (mut last, mut still) = (None, 0);
        for _ in 0..200 {
            h.run_steps(1);
            let at = h.query_by_label(name).map(|n| n.rect());
            still = if at.is_some() && at == last { still + 1 } else { 0 };
            if still >= 3 {
                break;
            }
            last = at;
            pause();
        }
        if h.query_by_label(name).is_some() {
            h.get_by_label(name).click();
        }
        h.run_steps(3);
    }
    assert_ne!(option(h, name), before, "{name} could not be ticked");
    settle(h);
}

fn toggle_accents(h: &mut Harness<'static, PdfCraftApp>) {
    tick(h, "Accent-sensitive");
}

#[test]
fn find_matches_accented_and_plain_spellings_until_accent_sensitive_is_ticked() {
    let mut h = harness("cafe");
    assert_eq!(pages(&h), [0, 1, 2], "cafe finds Café, cafe and CAFÉ");
    toggle_accents(&mut h);
    assert!(h.state().views[0].find.as_ref().unwrap().accent_sensitive);
    assert_eq!(pages(&h), [1], "accent-sensitive: only the plain spelling");
    toggle_accents(&mut h);
    assert!(!h.state().views[0].find.as_ref().unwrap().accent_sensitive);
    assert_eq!(pages(&h), [0, 1, 2], "and back");
}

#[test]
fn an_accented_query_finds_the_plain_spelling_too_by_default() {
    let mut h = harness("café");
    assert_eq!(pages(&h), [0, 1, 2]);
    toggle_accents(&mut h);
    assert_eq!(pages(&h), [0, 2], "accent-sensitive, case still ignored: Café and CAFÉ");
}

#[test]
fn the_accent_option_is_independent_of_case() {
    let mut h = harness("Cafe");
    // Case-sensitive: "Cafe" is not "cafe" or "CAFÉ", but it is "Café" once accents are ignored.
    tick(&mut h, "Case-sensitive");
    assert_eq!(pages(&h), [0], "Café matches Cafe: the accent is ignored, the capital is not");
    toggle_accents(&mut h);
    assert!(pages(&h).is_empty(), "with accents exact, Cafe is none of them");
}
