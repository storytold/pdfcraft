//! Work counts on a 500-page document (parity view.large-document-performance): scrolling and wheel frames build only
//! the pages and rows in view, never the whole document. Each assertion is a relation between counts of the same
//! script on a small and a large document, from the component runs the framework counts, so a slow or busy machine
//! cannot fail it and a regression cannot hide behind a fast one. Rendering runs on worker threads and is not counted.

use craft::pages::ui::view::GoTo;
use craft::ui::testing::last_pass_runs;
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

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

/// The app with `bytes` open and each of `options` set, stepped at 60 frames a second.
fn harness(bytes: Vec<u8>, options: &[(&str, &str)]) -> Harness<'static, PdfCraftApp> {
    let options: Vec<(String, String)> = options.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "work.pdf", None, bytes).expect("opens");
            for (key, value) in options {
                app.set_option(&egui::Context::default(), &key, &value).unwrap();
            }
            app
        }
    });
    h.run_steps(8);
    h
}

/// The last path segment of a component's type name, before any generics.
fn short(name: &str) -> &str {
    let bare = name.split('<').next().unwrap_or(name);
    bare.rsplit("::").next().unwrap_or(bare)
}

/// What one frame cost: the runs of the component type `row` (every type when `None`) in the frame's last pass, and the
/// passes the frame ran.
fn frame_work(h: &mut Harness<'static, PdfCraftApp>, row: Option<&str>) -> (u32, u64) {
    let before = h.ctx.cumulative_pass_nr();
    h.step();
    let passes = h.ctx.cumulative_pass_nr().saturating_sub(before);
    let runs = last_pass_runs(&h.ctx).iter().filter(|(name, _)| row.is_none_or(|row| short(name) == row)).map(|(_, count)| *count).sum();
    (runs, passes)
}

/// The most runs and passes in one frame over `frames` frames, `each` run before every frame. A frame that replays a settled
/// region runs nothing, so the maximum over several frames is the work of the frames that did run.
fn most_work(
    h: &mut Harness<'static, PdfCraftApp>,
    row: Option<&str>,
    frames: usize,
    mut each: impl FnMut(&mut Harness<'static, PdfCraftApp>, usize),
) -> (u32, u64) {
    let mut most = (0, 0);
    for frame in 0..frames {
        each(h, frame);
        let (runs, passes) = frame_work(h, row);
        most = (most.0.max(runs), most.1.max(passes));
    }
    most
}

/// A folded page builds no comment cards, and an open one builds cards for the rows in view only, not for all 10,000 roots.
#[test]
fn a_folded_page_builds_no_comment_cards_and_an_open_one_builds_only_the_rows_in_view() {
    const ROOTS: u32 = 10_000;
    let mut h = harness(named_comments(ROOTS as usize), &[("left", "closed"), ("panel", "comments")]);
    h.state_mut().views[0].session.comments.panel.collapsed_pages = vec![0];
    {
        let state = h.state();
        let info = &state.session.get(state.views[0].id).unwrap().info;
        assert_eq!(info.pages.len(), 1);
        assert_eq!(info.annotations.len(), ROOTS as usize);
        for a in &info.annotations {
            assert_eq!(a.name.as_deref(), Some(format!("root-{}", a.index).as_str()));
            assert!(a.in_reply_to.is_none());
            assert!(a.state.is_none());
        }
        assert_eq!(state.views[0].session.comments.panel.collapsed_pages, [0]);
    }
    let (folded, _) = most_work(&mut h, Some("CommentRow"), 6, |_, _| {});
    assert_eq!(folded, 1, "a folded page builds only its heading row, no cards");
    h.state_mut().views[0].session.comments.panel.collapsed_pages.clear();
    let (open, _) = most_work(&mut h, Some("CommentRow"), 6, |_, _| {});
    assert!(open > 0, "the open page builds the cards in view");
    assert!(open < ROOTS, "{open} cards for {ROOTS} roots: only the rows in view are built");
}

/// Jumping through a long document costs work for the pages in view only: the work at 500 pages is no more than at 50.
#[test]
fn jumping_through_pages_builds_only_the_pages_in_view() {
    let work = |n: usize| {
        let mut h = harness(big(n), &[]);
        most_work(&mut h, None, 60, |h, f| h.state_mut().views[0].view.go_to = Some(GoTo::Top { page: (f * 5) / 2 % 50, fraction: 0.3 }))
    };
    let (small, large) = (work(50), work(500));
    assert!(small.0 > 0, "the frames build something");
    assert!(large.0 <= small.0, "component runs in one frame: {} at 500 pages, {} at 50", large.0, small.0);
    assert!(large.1 <= small.1, "passes in one frame: {} at 500 pages, {} at 50", large.1, small.1);
}

/// Wheel frames and the idle frames between them do not rebuild the pages of the whole document.
#[test]
fn wheel_frames_build_only_the_pages_in_view() {
    let work = |n: usize| {
        let mut h = harness(big(n), &[("left", "closed"), ("panel", "none"), ("layout", "continuous"), ("zoom", "200"), ("animation_effects", "on")]);
        h.hover_at(h.state().views[0].viewport_rect().center());
        h.run_steps(2);
        let start = h.state().views[0].current();
        let most = most_work(&mut h, None, 60, |h, frame| {
            if frame % 3 == 0 {
                h.event(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -1.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                    source: egui::MouseWheelSource::Unknown,
                });
            }
        });
        assert!(h.state().views[0].current() > start, "wheel events must actually move through the document");
        most
    };
    let (small, large) = (work(50), work(500));
    assert!(small.0 > 0, "the frames build something");
    assert!(large.0 <= small.0, "component runs in one frame: {} at 500 pages, {} at 50", large.0, small.0);
    assert!(large.1 <= small.1, "passes in one frame: {} at 500 pages, {} at 50", large.1, small.1);
}

/// The Comments, Pages and Fields panels build rows for the rows in view, not for every item: the most row runs in one
/// frame at 500 pages are no more than at 50.
#[test]
fn panels_build_only_the_rows_in_view() {
    // The Fields panel has no field in this document, so only the relation holds there; the others must build rows.
    for (panel, row, builds) in [("comments", "CommentRow", true), ("pages", "Tile", true), ("fields", "ListRow", false)] {
        let work = |n: usize| {
            let mut h = harness(big(n), &[("panel", panel)]);
            most_work(&mut h, Some(row), 60, |h, f| h.state_mut().views[0].view.go_to = Some(GoTo::Top { page: (f * 5) % 50, fraction: 0.3 })).0
        };
        let (small, large) = (work(50), work(500));
        if builds {
            assert!(small > 0, "{panel}: the {row} rows are built");
        }
        assert!(large <= small, "{panel}: {large} {row} runs in one frame at 500 pages, {small} at 50");
    }
}

/// An app on the default host with test services: no wake-ups, no autosave folder, the GitHub update
/// source, a fresh intake and an inline watch (as `tests/it/control.rs`' `new_app`).
fn new_app() -> pdfcraft_ui_egui::PdfCraftApp {
    use craft::host::{AppInfo, Host, Wake};
    use craft::platform::clipboard::{Clipboard, SystemClipboard};
    use craft::platform::open::FileIntake;
    use craft::platform::watch::FileWatch;
    let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "test" }).build();
    let services = pdfcraft_ui_egui::AppServices::new(
        Wake::none(),
        Clipboard::new(SystemClipboard::new()),
        None,
        None,
        FileIntake::new(Wake::none()).0,
        FileWatch::new(Wake::none()).inline(),
    );
    pdfcraft_ui_egui::PdfCraftApp::with_host(std::sync::Arc::new(host), services)
}

/// Installs the framework runtime on a harness context, as `tests/it/support`'s `install_runtime` (L-227).
fn install_runtime(ctx: &egui::Context) {
    let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), std::sync::Arc::new(craft::window::system::NoWindows));
    craft::ui::shell::embed::install(ctx, spec, &pdfcraft_ui_egui::fonts::ui_fonts(pdfcraft_ui_egui::i18n::Lang::EN))
        .expect("the runtime installs on a fresh context");
}
