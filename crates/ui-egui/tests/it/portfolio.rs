//! A PDF portfolio (`/Collection`, ISO 32000-2 §7.11.6) opens on its documents, not on the cover
//! sheet its pages are (#1652): the document `/D` names opens in a tab of its own, and the
//! portfolio's tab shows its file list even with the right panel closed.

use std::sync::{Arc, Mutex};

use crate::support::AppHarness as _;
use craft::host::{ManualJobs, ScriptedFiles};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

/// A one-page PDF.
const PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A one-page PDF with a comment on its page.
const NOTED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /Contents (Sign here) /T (HR) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// [`PAGE`] encrypted, opening with the password `pw`.
fn locked() -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(PAGE.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "pw",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// A one-page portfolio of `files` (name, bytes), with `catalog` (its `/Collection` and any other
/// entries) in its catalog.
fn portfolio(catalog: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let names: String = files.iter().enumerate().map(|(i, (name, _))| format!("({name}) {} 0 R ", 5 + 2 * i)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} /Names << /EmbeddedFiles << /Names [{names}] >> >> >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_vec(),
    ];
    for (i, (name, data)) in files.iter().enumerate() {
        objs.push([format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat());
        objs.push(format!("<< /Type /Filespec /F ({name}) /UF ({name}) /EF << /F {} 0 R >> >>", 4 + 2 * i).into_bytes());
    }
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

/// A harness whose app is built by `setup` on a host with held jobs and picks, and the jobs.
fn harness_with(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, Arc<ManualJobs>) {
    let jobs = Arc::new(ManualJobs::new());
    let parts = Mutex::new(Some((jobs.clone(), setup)));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let (jobs, setup) = parts.lock().unwrap().take().unwrap();
            let mut app = crate::support::test_app(crate::support::host_with(jobs, Arc::new(ScriptedFiles::held())));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    (h, jobs)
}

/// The app with `pdf` opened as `portfolio.pdf`, the reader keeping the right panel closed or not.
fn opened(pdf: Vec<u8>, panel_closed: bool) -> Harness<'static, PdfCraftApp> {
    harness_with(move |app| {
        if panel_closed {
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
        }
        app.open_bytes(&egui::Context::default(), "portfolio.pdf", None, pdf).unwrap();
    })
    .0
}

/// The open tabs' names, and the active one's.
fn tabs(h: &Harness<'static, PdfCraftApp>) -> (Vec<String>, Option<String>) {
    let s = h.state();
    let name = |i: usize| s.views.get(i).and_then(|v| s.session.get(v.id)).map(|d| d.name.clone());
    ((0..s.views.len()).filter_map(name).collect(), s.active.and_then(name))
}

/// Make the tab at `index` the active one by clicking its name in the tab strip.
fn select(h: &mut Harness<'static, PdfCraftApp>, index: usize) {
    let name = tabs(h).0[index].clone();
    // The tab strip is the topmost node with the document's name; a file list may repeat it below.
    let tab = h.get_all_by_label(&name).min_by(|a, b| a.rect().top().total_cmp(&b.rect().top())).expect("the tab is on screen");
    tab.click();
    h.run_steps(2);
}

/// Whether the dock has the side panel `panel` open; its rail icon shows whether or not it is.
fn panel_shown(h: &Harness<'static, PdfCraftApp>, panel: RightPanel) -> bool {
    crate::support::right(h.state()) == Some(panel)
}

/// The reader picks the right panel `panel`, or closes it with `None`.
fn choose_panel(h: &mut Harness<'static, PdfCraftApp>, panel: Option<RightPanel>) {
    h.state_mut().choose_right_panel(panel);
    h.run_steps(2);
}

/// Answer the password dialog: type `password` and press Open, or press Cancel.
fn answer_password(h: &mut Harness<'static, PdfCraftApp>, password: Option<&str>) {
    // PLAN: the dialog's field and button labels are assumed (framework password pattern).
    match password {
        Some(password) => {
            h.get_all_by_label("Password").last().expect("the password dialog is on screen").type_text(password);
            h.run_steps(1);
            h.get_all_by_label("Open").last().expect("the Open button").click();
        }
        None => h.get_all_by_label("Cancel").last().expect("the Cancel button").click(),
    }
    h.run_steps(2);
}

/// The initial document opens in front, and the portfolio's tab lists its files whatever the
/// reader's panel preference, giving the panel back when left.
#[test]
fn a_portfolio_opens_its_initial_document_and_shows_its_files_with_the_panel_closed() {
    let mut h = opened(portfolio("/Collection << /D (form.pdf) /View /T >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]), true);
    // The document `/D` names is presented, in a tab of its own, with the reader's own panels.
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "form.pdf".into()], Some("form.pdf".into())));
    assert!(!panel_shown(&h, RightPanel::Attachments));
    // The portfolio's tab shows its files, without changing the reader's preference.
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
    // Leaving it gives back the closed panel; coming back shows the files again.
    select(&mut h, 1);
    assert!(!panel_shown(&h, RightPanel::Attachments));
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
    // The reader can still close the list on the portfolio's tab.
    choose_panel(&mut h, None);
    assert!(!panel_shown(&h, RightPanel::Attachments));
    // A panel the reader had open comes back once they leave the portfolio's tab.
    select(&mut h, 1);
    choose_panel(&mut h, Some(RightPanel::Bookmarks));
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
    select(&mut h, 1);
    assert!(panel_shown(&h, RightPanel::Bookmarks));
}

/// The initial document opens on its own panels, not on the portfolio's file list.
#[test]
fn the_initial_document_opens_without_the_portfolios_file_list() {
    // The portfolio asks for the Attachments panel too (`/PageMode /UseAttachments`), and the
    // reader keeps the right panel open: the document it presents opens with the panels any other
    // would, here on its comments, not with the portfolio's file list.
    let mut h = opened(portfolio("/PageMode /UseAttachments /Collection << /D (form.pdf) >>", &[("intro.pdf", PAGE), ("form.pdf", NOTED)]), false);
    assert_eq!(tabs(&h).1.as_deref(), Some("form.pdf"));
    assert!(panel_shown(&h, RightPanel::Comments));
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
}

/// A file opened together with a portfolio, after it, stays in front.
#[test]
fn a_file_opened_after_the_portfolio_keeps_the_front() {
    // Two files opened at once, the portfolio first: the second stays in front, and the
    // portfolio's initial document waits for the reader, whose list shows on the portfolio's tab.
    let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("form.pdf", PAGE)]);
    let (mut h, _jobs) = harness_with(move |app| {
        app.set_option(&egui::Context::default(), "panel", "none").unwrap();
        app.open_bytes(&egui::Context::default(), "portfolio.pdf", None, pdf).unwrap();
        app.open_bytes(&egui::Context::default(), "other.pdf", None, PAGE.to_vec()).unwrap();
    });
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "other.pdf".into()], Some("other.pdf".into())));
    assert!(!panel_shown(&h, RightPanel::Attachments));
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
}

/// A password asked for a file opened together with the portfolio stays asked: the initial
/// document waits for the answer, then gives way to that file, or opens once the prompt is
/// cancelled.
#[test]
fn a_password_asked_for_a_file_opened_with_the_portfolio_stays_asked() {
    for answer in [Some("pw"), None] {
        let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("form.pdf", PAGE)]);
        let (mut h, jobs) = harness_with(move |app| {
            app.open_bytes(&egui::Context::default(), "portfolio.pdf", None, pdf).unwrap();
            app.open_bytes(&egui::Context::default(), "locked.pdf", None, locked()).unwrap();
        });
        assert!(h.query_all_by_label("Password").next().is_some(), "the prompt for locked.pdf is still up");
        assert_eq!(tabs(&h).0, ["portfolio.pdf"]);
        let opened = if answer.is_some() { "locked.pdf" } else { "form.pdf" };
        answer_password(&mut h, answer);
        // The answered file opens through the queue.
        jobs.run_all();
        crate::support::wait_until(&mut h, "a second tab", |app| app.views.len() == 2);
        h.run_steps(4);
        assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), opened.into()], Some(opened.into())));
    }
}

/// An attachment deleted while the initial document waits (here for a password asked for a file
/// opened with the portfolio) doesn't change which document opens.
#[test]
fn a_deleted_attachment_doesnt_change_which_document_opens() {
    let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]);
    let (mut h, _jobs) = harness_with(move |app| {
        app.open_bytes(&egui::Context::default(), "portfolio.pdf", None, pdf).unwrap();
        app.open_bytes(&egui::Context::default(), "locked.pdf", None, locked()).unwrap();
    });
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeleteAttachment { key: "intro.pdf".into() }));
    answer_password(&mut h, None);
    h.run_steps(4);
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "form.pdf".into()], Some("form.pdf".into())));
}

/// Leaving the portfolio's tab in Read mode: Read mode gives back the panel the file list
/// replaced, not the list, on the tab that is active when it ends.
#[test]
fn read_mode_gives_back_the_panel_the_file_list_replaced() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("form.pdf", PAGE)]), true);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, PAGE.to_vec()).unwrap();
    h.run_steps(2);
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
    assert!(h.execute("view.read_mode"));
    h.run_steps(2);
    assert!(!panel_shown(&h, RightPanel::Attachments));
    select(&mut h, 1);
    assert!(h.execute("view.read_mode"));
    h.run_steps(2);
    assert!(!panel_shown(&h, RightPanel::Attachments), "the panel was closed before the list showed");
}

/// A panel the reader picks in Read mode on the portfolio's tab stays when they switch tabs.
#[test]
fn a_panel_picked_in_read_mode_stays_when_the_tab_changes() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("form.pdf", PAGE)]), true);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, PAGE.to_vec()).unwrap();
    h.run_steps(2);
    select(&mut h, 0);
    assert!(h.execute("view.read_mode"));
    h.run_steps(2);
    choose_panel(&mut h, Some(RightPanel::Attachments));
    select(&mut h, 1);
    assert!(panel_shown(&h, RightPanel::Attachments));
}

/// `/View /H` presents the initial document but leaves the file list to the reader.
#[test]
fn a_portfolio_that_keeps_its_list_hidden_still_presents_its_initial_document() {
    let mut h = opened(portfolio("/Collection << /D (form.pdf) /View /H >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]), true);
    assert_eq!(tabs(&h).1.as_deref(), Some("form.pdf"));
    select(&mut h, 0);
    assert!(!panel_shown(&h, RightPanel::Attachments), "/View /H: the list shows when the reader asks for it");
}

/// Without a PDF to present, the portfolio stays in front with its files listed.
#[test]
fn a_portfolio_without_an_initial_document_shows_its_files_over_the_cover() {
    // Without `/D` the cover is the initial document; a `/D` naming a file that isn't a PDF
    // opens nothing either.
    for collection in ["/Collection << /View /D >>", "/Collection << /D (notes.txt) >>"] {
        let h = opened(portfolio(collection, &[("notes.txt", b"hello"), ("form.pdf", PAGE)]), true);
        assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())), "{collection}");
        assert!(panel_shown(&h, RightPanel::Attachments), "{collection}");
    }
}

/// A portfolio nested in the initial document doesn't open a chain of tabs.
#[test]
fn a_portfolio_inside_the_initial_document_waits_for_the_reader() {
    // The initial document is itself a portfolio presenting a third file: opening the outer
    // one opens one tab, not a chain of them.
    let inner = portfolio("/Collection << /D (deep.pdf) >>", &[("deep.pdf", PAGE)]);
    let mut h = opened(portfolio("/Collection << /D (inner.pdf) >>", &[("inner.pdf", &inner)]), true);
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "inner.pdf".into()], Some("inner.pdf".into())));
    // Its own tab still shows its files.
    select(&mut h, 0);
    assert!(panel_shown(&h, RightPanel::Attachments));
}
