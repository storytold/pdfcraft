//! The frame-time budget applies to a 500-page document (parity view.large-document-performance).
//! The budget scales with the measured duration of a fixed integer workload on slow machines.
//! Rendering runs on worker threads and is not part of the measurement.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// The one-minute load average, where the OS reports it (macOS `vm.loadavg`, Linux
/// `/proc/loadavg`).
fn load_average() -> Option<f64> {
    if let Ok(s) = std::fs::read_to_string("/proc/loadavg") {
        return s.split_whitespace().next()?.parse().ok();
    }
    let out = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace()).split_whitespace().next()?.parse().ok()
}

/// Frame budgets only mean something on a machine that isn't overcommitted: with the load
/// average above twice the core count (other builds running), skip and say so.
fn overloaded() -> bool {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get) as f64;
    match load_average() {
        Some(l) if l > 2.0 * cores => {
            eprintln!("PERF skipped: load average {l:.0} on {cores} cores; frame budgets need a quiet machine");
            true
        }
        _ => false,
    }
}

/// The budget tests take turns: each starts render workers, and measuring one while another
/// runs would charge it for the other's work on a small CI machine.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Base budget for debug builds. Calibration can increase it by at most three times.
const BUDGET: std::time::Duration = std::time::Duration::from_millis(40);
const CALIBRATION_REFERENCE: std::time::Duration = std::time::Duration::from_millis(17);

fn budget_scale(calibration: std::time::Duration) -> f64 {
    (calibration.as_secs_f64() / CALIBRATION_REFERENCE.as_secs_f64()).clamp(1.0, 3.0)
}

#[test]
fn calibration_scales_slow_machines_without_exceeding_the_cap() {
    assert_eq!(budget_scale(std::time::Duration::from_millis(8)), 1.0);
    assert_eq!(budget_scale(std::time::Duration::from_millis(17)), 1.0);
    assert_eq!(budget_scale(std::time::Duration::from_micros(42_500)), 2.5);
    assert_eq!(budget_scale(std::time::Duration::from_millis(64)), 3.0);
}

/// Measure 18 million integer updates, taking the fastest of three trials.
fn calibration_average() -> std::time::Duration {
    let mut best = std::time::Duration::MAX;
    for _ in 0..3 {
        let mut value = std::hint::black_box(0x9e3779b97f4a7c15_u64);
        let start = std::time::Instant::now();
        for _ in 0..18_000_000 {
            value = value.wrapping_mul(0xd1342543de82ef95).rotate_left(17).wrapping_add(0x94d049bb133111eb);
        }
        std::hint::black_box(value);
        best = best.min(start.elapsed());
    }
    best
}

/// Measure calibration just before each frame round. The lowest average-to-budget ratio determines the result.
/// Later rounds run only when the measured average exceeds that round's calibrated budget.
fn best_average(label: &str, frames: usize, mut step: impl FnMut(usize)) -> (std::time::Duration, std::time::Duration) {
    let mut best = std::time::Duration::MAX;
    let mut best_budget = BUDGET;
    let mut best_ratio = f64::INFINITY;
    for round in 1..=3 {
        let calibration = calibration_average();
        let scale = budget_scale(calibration);
        let budget = BUDGET.mul_f64(scale);
        let start = std::time::Instant::now();
        let mut worst = std::time::Duration::ZERO;
        for f in 0..frames {
            let t = std::time::Instant::now();
            step(f);
            worst = worst.max(t.elapsed());
        }
        let avg = start.elapsed() / frames as u32;
        eprintln!(
            "PERF {label} round {round}: calibration {calibration:?}, factor {scale:.2}, budget {budget:?}, avg frame {avg:?}, worst {worst:?}"
        );
        let ratio = avg.as_secs_f64() / budget.as_secs_f64();
        if ratio < best_ratio {
            best = avg;
            best_budget = budget;
            best_ratio = ratio;
        }
        if avg < budget {
            break;
        }
    }
    (best, best_budget)
}

/// `n` text pages with a few comments each, so panels and overlays have work to do.
fn big(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 3 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 612 792] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [{} 0 R] >>",
            5 + 3 * i,
            6 + 3 * i
        ));
        let body: String =
            (0..40).map(|l| format!("BT /F1 10 Tf 72 {} Td (Page {} line {l} with some words to lay out) Tj ET\n", 720 - l * 15, i + 1)).collect();
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
        objs.push("<< /Type /Annot /Subtype /Square /Rect [72 72 144 144] /C [1 0 0] /T (Ada) /Contents (Check) >>".into());
    }
    pdf(objs)
}

fn pdf(objs: Vec<String>) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Named roots, none with replies: the panel still looks up every root's thread.
fn named_comments(n: usize) -> Vec<u8> {
    let annots: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + 4)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Annots [{}] >>", annots.join(" ")),
    ];
    for i in 0..n {
        objs.push(format!(
            "<< /Type /Annot /Subtype /Square /Rect [72 72 144 144] /C [1 0 0] /T (Ada) /NM (root-{i}) /Contents (Review note {i}) >>"
        ));
    }
    pdf(objs)
}

#[test]
fn collapsed_comments_with_ten_thousand_named_roots_stay_within_the_frame_budget() {
    if overloaded() {
        return;
    }
    let _turn = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = named_comments(10_000);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("review.pdf", None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "comments").unwrap();
        app.views[0].comments.collapsed_pages = vec![0];
        app
    });
    h.run_steps(4);
    {
        let state = h.state();
        let info = &state.session.get(state.views[0].id).unwrap().info;
        assert_eq!(info.pages.len(), 1);
        assert_eq!(info.annotations.len(), 10_000);
        for a in &info.annotations {
            assert_eq!(a.name.as_deref(), Some(format!("root-{}", a.index).as_str()));
            assert!(a.in_reply_to.is_none());
            assert!(a.state.is_none());
        }
        assert_eq!(state.views[0].comments.collapsed_pages, [0]);
    }
    // Folding removes card layout cost, so this measures filtering and thread lookup work.
    let (avg, budget) = best_average("collapsed named comments", 30, |_| h.step());
    assert!(avg < budget, "collapsed comments: average frame {avg:?}, budget {budget:?}");
}

#[test]
fn scrolling_a_500_page_document_stays_within_the_frame_budget() {
    if overloaded() {
        return;
    }
    let _turn = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = big(500);
    let t0 = std::time::Instant::now();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("big.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    eprintln!("PERF open {:?}", t0.elapsed());
    // Scroll through the document: 200 frames, jumping ~2 pages per frame.
    let (avg, budget) = best_average("scroll", 200, |f| {
        h.state_mut().views[0].goto = Some(((f * 5) / 2 % 500, 0.3));
        h.step();
    });
    assert!(avg < budget, "average frame {avg:?}, budget {budget:?}");
}

/// Measures real wheel events and their idle frames, separately from page-jump scheduling.
#[test]
fn regular_wheel_input_on_500_pages_stays_within_the_frame_budget() {
    if overloaded() {
        return;
    }
    let _turn = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = big(500);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("wheel-budget.pdf", None, bytes).expect("opens");
        for (key, value) in [("left", "closed"), ("panel", "none"), ("layout", "continuous"), ("zoom", "200"), ("animation_effects", "on")] {
            app.set_option(key, value).unwrap();
        }
        app
    });
    h.run_steps(8);
    h.hover_at(h.state().views[0].viewport_rect().center());
    h.run_steps(2);
    let start = h.state().views[0].current;
    let average = best_average("wheel input", 200, |frame| {
        if frame % 3 == 0 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Unknown,
            });
        }
        h.step();
    });
    assert!(h.state().views[0].current > start, "wheel events must actually move through the document");
    assert!(average < BUDGET, "wheel input: average frame {average:?}");
}

#[test]
fn panels_with_hundreds_of_items_stay_within_the_frame_budget() {
    if overloaded() {
        return;
    }
    let _turn = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = big(500);
    for panel in ["comments", "pages", "fields"] {
        let b = bytes.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("big.pdf", None, b).expect("opens");
            app.set_option("panel", panel).unwrap();
            app
        });
        h.run_steps(4);
        let (avg, budget) = best_average(panel, 100, |f| {
            h.state_mut().views[0].goto = Some(((f * 5) % 500, 0.3));
            h.step();
        });
        assert!(avg < budget, "{panel}: average frame {avg:?}, budget {budget:?}");
    }
}
