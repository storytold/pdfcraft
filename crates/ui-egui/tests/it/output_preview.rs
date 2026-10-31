//! Print production ▸ Output preview: the panel lists the page's inks and reads their coverage
//! under the pointer, with and without overprint simulation, and names what the process plates
//! simulate.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

#[path = "../../../render/tests/support/synthetic_icc.rs"]
mod synthetic_icc;

/// One 300 × 400 pt page: solid black, and a spot colour ("Gold" at 50%) overprinting it in a
/// 100 × 100 pt square in the middle.
fn fixture() -> Vec<u8> {
    pdf("", &[])
}

/// The fixture's page with more catalog entries in `catalog` and `extra` objects from 5.
fn pdf(catalog: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let content = "0 0 0 1 k 0 0 300 400 re f /GS1 gs /CS0 cs 0.5 scn 100 150 100 100 re f";
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /ColorSpace << /CS0 [/Separation /Gold /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.3 1 0] /N 1 >>] >> /ExtGState << /GS1 << /op true >> >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    objs.extend_from_slice(extra);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The fixture printing on the synthetic press of `synthetic_icc`, its catalog's output intent.
fn pressed() -> Vec<u8> {
    let profile = synthetic_icc::cmyk_profile("PdfCraft synthetic press", true);
    let mut stream = format!("<< /N 4 /Length {} >>\nstream\n", profile.len()).into_bytes();
    stream.extend_from_slice(&profile);
    stream.extend_from_slice(b"\nendstream");
    pdf("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /DestOutputProfile 5 0 R >>]", &[stream])
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(fixture())
}

fn harness_with(document: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("inks.pdf", None, document).unwrap();
        // Not 100%: the zoom box would read "100%", like an ink.
        app.set_option("zoom", "125").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// How many elements are labelled `label`.
fn count(h: &Harness<'static, PdfCraftApp>, label: &str) -> usize {
    h.query_all_by_label(label).count()
}

/// Step frames, pointing at `at`, until `ready` (the page separates on a worker).
fn wait_until(h: &mut Harness<'static, PdfCraftApp>, at: Option<Pos2>, what: &str, ready: impl Fn(&Harness<'static, PdfCraftApp>) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(p) = at {
            h.hover_at(p);
        }
        h.step();
        if ready(h) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The middle of the page, inside the spot colour square.
fn middle(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    h.state().views[0].page_screen_rect(0).unwrap().center()
}

#[test]
fn output_preview_lists_inks_and_reads_coverage_under_the_pointer() {
    let mut h = harness();
    h.state_mut().run_command("prepress.output_preview");
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::OutputPreview));
    wait_until(&mut h, None, "the inks", |h| count(h, "Gold") == 1);
    for ink in ["Cyan", "Magenta", "Yellow", "Black"] {
        assert_eq!(count(&h, ink), 1, "{ink}");
    }
    assert_eq!(count(&h, "Simulation: device formulas"), 1, "the document has no output intent");
    // The spot colour overprints the black: both print under the pointer (Black 100%, Gold
    // 50%, 150% in total).
    let p = middle(&h);
    wait_until(&mut h, Some(p), "the coverage under the pointer", |h| count(h, "150%") == 1);
    assert_eq!((count(&h, "100%"), count(&h, "50%"), count(&h, "0%")), (1, 1, 3));
    assert_eq!(count(&h, "Page maximum: 150%"), 1);

    // Without overprint simulation the spot colour knocks the black out: Gold 50% is the total.
    h.get_by_label("Simulate overprinting").click();
    wait_until(&mut h, Some(p), "the knocked-out square", |h| count(h, "Page maximum: 100%") == 1 && count(h, "50%") == 2);
    assert_eq!((count(&h, "150%"), count(&h, "100%"), count(&h, "0%")), (0, 0, 4));

    // Switching an ink off leaves it out of the total, and its own coverage still reads.
    h.get_by_label("Gold").click();
    wait_until(&mut h, Some(p), "Gold left out", |h| count(h, "50%") == 1 && count(h, "0%") == 5);
}

#[test]
fn output_preview_names_the_output_intent_it_simulates() {
    let mut h = harness_with(pressed());
    h.state_mut().run_command("prepress.output_preview");
    h.run_steps(2);
    wait_until(&mut h, None, "the separation", |h| count(h, "Gold") == 1);
    assert_eq!(count(&h, "Simulation: PdfCraft synthetic press"), 1);
    assert_eq!(count(&h, "Simulation: device formulas"), 0);
}
