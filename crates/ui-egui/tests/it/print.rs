//! The Print dialog in the real shell (egui_kittest): settings, preview sheets, Save as PDF.
//!
//! The app runs on a host whose jobs the test runs (`ManualJobs`) and whose files answer from a
//! script (`ScriptedFiles`), so a read is pending until the test runs it and a Save pick is
//! answered by the script. The dialog is the hosted `HostedPrint`; a test drives it as automation
//! does (`ui.dialog.set`, its action ids, its OK) and reads the result through its `State`.

use crate::support::AppHarness as _;
use craft::host::{ManualJobs, Picked, ScriptedFiles};
use craft::print::ui::{Destination, HostedPrint};
use craft_ui::dialog::{DialogId, HeldOk};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::print::spool;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// PLAN: assumed `Host::builder(AppInfo).jobs(..).files(..).build()`, `PdfCraftApp::with_host(Arc<Host>, AppServices)`,
// `AppServices::new(wake, intake, watch)` (q171, q184, q187), `FileIntake::new(Wake)`, `FileWatch::inline()`, the dialog's
// listed `notes` (the note rows' keys) and `sheet`/`sheets` (the pager), `ScriptedFiles::failing_writes(path)` (F-4),
// `PdfCraftApp::{apply_settings, settings_snapshot}` over `pdfcraft_ui_egui::settings`
// (`import_json(&str) -> (Settings, _)`, `Settings: Serialize`, `Settings::print.{paper, paper_overridden}`),
// `PdfCraftApp::dialogs` public, and `read_options_in_background` callable from tests (S-3). w4 note 12
// names the `range` field and the action ids; the `printer` and `paper` field keys are assumed.

/// The app on a host the test drives: the harness, the jobs it runs by hand and the scripted files.
pub(crate) struct Rig {
    h: Harness<'static, PdfCraftApp>,
    /// The host's jobs: nothing runs until the test runs it.
    pub(crate) jobs: Arc<ManualJobs>,
    /// The host's files: Save picks are answered from the script.
    pub(crate) files: Arc<ScriptedFiles>,
}

impl std::ops::Deref for Rig {
    type Target = Harness<'static, PdfCraftApp>;
    fn deref(&self) -> &Self::Target {
        &self.h
    }
}

impl std::ops::DerefMut for Rig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.h
    }
}

/// An app on a scripted host, prepared by `setup`.
pub(crate) fn rig_on(files: ScriptedFiles, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Rig {
    let jobs = Arc::new(ManualJobs::new());
    let files = Arc::new(files);
    let (host_jobs, host_files) = (jobs.clone(), files.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(crate::support::host_with(host_jobs, host_files));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    Rig { h, jobs, files }
}

/// Restores saved settings given as JSON, as a start of the app reads them.
pub(crate) fn restore(app: &mut PdfCraftApp, json: &str) {
    app.apply_settings(&pdfcraft_ui_egui::settings::import_json(json).0);
}

/// The settings the app would save now, as JSON.
fn saved_json(app: &PdfCraftApp) -> String {
    serde_json::to_string(&app.settings_snapshot()).unwrap()
}

/// The answers for the Save as PDF picks to come: `count` files, `out0.pdf`, `out1.pdf`, ...
fn saves(count: usize) -> ScriptedFiles {
    ScriptedFiles::answering((0..count).map(|i| Picked::Paths(vec![out(i)])))
}

/// The path the `i`-th scripted Save answers with.
fn out(i: usize) -> PathBuf {
    PathBuf::from(format!("out{i}.pdf"))
}

pub(crate) fn harness() -> Rig {
    harness_with_settings(None)
}

pub(crate) fn harness_with_settings(settings: Option<String>) -> Rig {
    harness_saving(settings, 1)
}

/// The app started again on the preferences `saved` left, as the settings store hands them back, with the form open.
fn harness_restarted(saved: pdfcraft_ui_egui::settings::PdfCraftSettings) -> Rig {
    rig_on(saves(1), move |app| {
        app.apply_settings(&saved);
        app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    })
}

fn harness_saving(settings: Option<String>, saves_count: usize) -> Rig {
    rig_on(saves(saves_count), move |app| {
        if let Some(json) = &settings {
            restore(app, json);
        }
        app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    })
}

/// A listed printer, `default` or not, that has not said whether it prints two-sided.
pub(crate) fn printer(name: &str, default: bool) -> spool::Printer {
    spool::Printer { name: name.into(), default, two_sided: None, description: String::new() }
}

impl Rig {
    /// Opens the Print dialog.
    pub(crate) fn open(&mut self) {
        assert!(self.execute("print.dialog"));
        self.run_steps(3);
    }

    /// The open Print dialog.
    pub(crate) fn hosted(&self) -> &HostedPrint {
        self.state().dialogs.get::<HostedPrint>().expect("the Print dialog is open")
    }

    /// The open Print dialog, mutably.
    pub(crate) fn hosted_mut(&mut self) -> &mut HostedPrint {
        self.h.state_mut().dialogs.get_mut::<HostedPrint>().expect("the Print dialog is open")
    }

    /// Whether the Print dialog is open.
    pub(crate) fn is_open(&self) -> bool {
        self.state().dialogs.is_open::<HostedPrint>()
    }

    /// The Print dialog's id, as `ui.inspect` lists it.
    fn id(&self) -> DialogId {
        self.state().dialogs.open_dialogs().last().expect("a dialog is open").id
    }

    /// Closes the dialog as Cancel does.
    pub(crate) fn close(&mut self) {
        let ctx = self.ctx.clone();
        self.h.state_mut().dialogs.cancel::<HostedPrint>(&ctx);
        self.run_steps(2);
    }

    /// Whether a printer list or a paper read is pending, or a job waits to run.
    fn pending(&self) -> bool {
        self.hosted().state().discovering() || self.state().print_draft.papers_loading() || self.jobs.waiting() > 0
    }

    /// Runs the host's jobs and frames until nothing is pending, then lets the dialog settle.
    pub(crate) fn settle(&mut self) {
        for _ in 0..200 {
            self.jobs.run_all();
            self.run_steps(1);
            if !self.is_open() || !self.pending() {
                break;
            }
        }
        assert!(!self.is_open() || !self.pending(), "the dialog settled");
        self.run_steps(4);
    }

    /// Runs the host's jobs one at a time until the printer list has arrived, leaving the reads it
    /// starts pending.
    fn list(&mut self) {
        for _ in 0..50 {
            if !self.hosted().state().discovering() {
                break;
            }
            self.jobs.run_next();
            self.run_steps(1);
        }
        assert!(!self.hosted().state().discovering(), "the printer list arrived");
        self.run_steps(2);
    }

    /// Closes the dialog and opens it again with its lists loaded.
    pub(crate) fn reopen(&mut self) {
        self.close();
        self.open();
        self.settle();
    }

    /// Sets a field of the open dialog as `ui.dialog.set` does.
    pub(crate) fn set(&mut self, field: &str, value: serde_json::Value) {
        let (ctx, id) = (self.ctx.clone(), self.id());
        self.h.state_mut().dialogs.set_json_field(&ctx, id, field, value).unwrap_or_else(|e| panic!("{field}: {e}"));
        self.run_steps(2);
    }

    /// Picks the destination as the Printer list does: a printer, or Save as PDF for `None`.
    pub(crate) fn pick(&mut self, printer: Option<&str>) {
        self.set("printer", printer.map_or(json!(null), |name| json!(name)));
    }

    /// Presses a button of the dialog by its action id.
    pub(crate) fn action(&mut self, id: &str) {
        self.hosted_mut().requests_mut().request_action(id.to_owned());
        self.run_steps(2);
    }

    /// The destination the dialog has chosen.
    pub(crate) fn destination(&self) -> Destination {
        self.hosted().state().settings.destination.clone()
    }

    /// Presses OK and runs frames, so the app's pass answers it. `None` once the dialog has closed.
    pub(crate) fn ok(&mut self) -> Option<HeldOk> {
        let (ctx, id) = (self.ctx.clone(), self.id());
        self.h.state_mut().dialogs.confirm_by_id(&ctx, id);
        self.run_steps(3);
        self.state().dialogs.held_state(id)
    }

    /// Runs the jobs and frames a Print to a printer needs to reach the spooler.
    fn run_print(&mut self) {
        self.jobs.run_all();
        self.run_steps(3);
        self.jobs.run_all();
        self.run_steps(2);
    }

    /// The dialog's listed fields, as `ui.inspect` gives them.
    fn listed(&self) -> serde_json::Value {
        self.state().dialogs.open_dialogs().last().and_then(|d| d.fields.clone()).unwrap_or(serde_json::Value::Null)
    }

    /// The sheet the pager shows and how many sheets there are.
    fn pager(&self) -> (u64, u64) {
        let fields = self.listed();
        (fields["sheet"].as_u64().unwrap_or(0), fields["sheets"].as_u64().unwrap_or(0))
    }

    /// Whether the dialog shows the note row with this key.
    pub(crate) fn has_note(&self, key: &str) -> bool {
        self.listed()["notes"].as_array().is_some_and(|notes| notes.iter().any(|n| n == key))
    }

    /// Whether the preview shows why the draft can't print a document of `pages` pages, as the draft gives the reason.
    fn preview_says_why(&self, pages: usize) -> bool {
        let why = self.state().print_draft.settings(pages, &[]).expect_err("the draft can't print");
        self.query_by_label_contains(&why).is_some()
    }

    /// Adds a custom paper through the custom row, as automation does: its fields, then its Add.
    pub(crate) fn add_custom(&mut self, name: &str, width: f64, length: f64) {
        self.set("custom_name", json!(name));
        self.set("custom_width", json!(width));
        self.set("custom_height", json!(length));
        self.action("custom.add");
    }

    /// What the `i`-th scripted Save wrote.
    fn saved(&self, i: usize) -> Arc<Vec<u8>> {
        Arc::new(self.files.file(Path::new(&out(i))).expect("saved"))
    }
}

/// Records the jobs that reach the spooler.
fn record_spooler(h: &mut Rig) -> Arc<Mutex<Vec<spool::Job>>> {
    let jobs: Arc<Mutex<Vec<spool::Job>>> = Arc::default();
    let sent = jobs.clone();
    h.state_mut().spooler = Some(Arc::new(move |_pdf: &[u8], job: &spool::Job| {
        sent.lock().unwrap().push(job.clone());
        Ok(String::new())
    }));
    jobs
}

/// A printer list of the named printers, the first the default, with no papers of their own.
fn list_printers(h: &mut Rig, names: &[&str]) {
    let listed: Vec<spool::Printer> = names.iter().enumerate().map(|(i, n)| printer(n, i == 0)).collect();
    h.state_mut().printer_source = Some(Arc::new(move || listed.clone()));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| Vec::new()));
    h.state_mut().default_paper_source = Some(Arc::new(|_| None));
}

#[test]
fn current_print_page_saves_the_viewed_page_when_labels_restart() {
    // Five distinct painted pages, labelled i, ii, iii, 1, 2.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /r >> 3 << /S /D /St 1 >>] >> >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R 9 0 R 11 0 R] /Count 5 /MediaBox [0 0 200 300] >>".to_owned(),
    ];
    for (i, color) in ["1 0 0", "0 1 0", "0 0 1", "1 1 0", "1 0 1"].iter().enumerate() {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Resources << >> /Contents {} 0 R >>", 4 + i * 2));
        let content = format!("{color} rg 0 0 200 300 re f");
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut h = rig_on(saves(2), move |app| {
        app.open_bytes(&egui::Context::default(), "labelled.pdf", None, bytes).unwrap();
    });
    let id = h.state().views[0].id;
    let labels: Vec<_> = h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["i", "ii", "iii", "1", "2"]);
    for (current, expected) in [[255, 0, 0], [0, 255, 0]].into_iter().enumerate() {
        h.set_option("page", &(current + 1).to_string()).unwrap();
        h.run_steps(3);
        h.open();
        h.state_mut().print_draft.which = pdfcraft_ui_egui::PrintWhich::Current;
        h.run_steps(3);
        assert_eq!(h.state().print_draft.current_page, current);
        assert_eq!(h.pager(), (1, 1));
        h.pick(None);
        h.ok();
        assert!(!h.is_open());
        let saved = h.saved(current);
        let doc = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
        let mut renderer = pdfcraft_render::PageRenderer::new(saved, Default::default());
        let shot = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(shot.error.is_none(), "{:?}", shot.error);
        let center = ((shot.height / 2 * shot.width + shot.width / 2) * 4) as usize;
        assert_eq!(&shot.rgba[center..center + 3], &expected, "saved the wrong physical page for {current}");
    }
}

#[test]
fn print_dialog_lays_out_sheets_and_saves_a_pdf() {
    let mut h = harness();
    h.open();
    assert!(h.is_open());
    assert_eq!(h.pager(), (1, 1));
    // Two copies of the one page, as a 2-up poster-free layout: Multiple.
    h.state_mut().print_draft.handling = pdfcraft_ui_egui::PrintHandling::Multiple;
    h.run_steps(2);
    assert_eq!(h.pager(), (1, 1));
    // No printer on a test machine: Save as PDF.
    h.pick(None);
    h.ok();
    assert!(!h.is_open());
    let doc = pdfcraft_cos::Document::open(h.saved(0)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
}

/// Pages picked beforehand are what the dialog offers to print (Acrobat's "Selected pages").
#[test]
fn print_dialog_prints_the_selected_pages() {
    use pdfcraft_ui_egui::PrintWhich;
    let mut h = harness();
    let bytes = h.state().session.create_blank(200.0, 300.0, 5).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "five.pdf", None, bytes.as_ref().clone()).unwrap();
    h.run_steps(3);
    let tab = h.state().views.len() - 1;
    // Nothing picked: the whole document, and no "Selected pages" choice.
    h.open();
    assert_eq!(h.state().print_draft.which, PrintWhich::All);
    assert!(h.state().print_draft.selected.is_empty(), "no Selected pages choice");
    assert_eq!(h.pager(), (1, 5));
    h.close();
    // Pages 2 and 4 picked.
    h.state_mut().views[tab].select_pages(&[1, 3]);
    h.open();
    assert_eq!(h.state().print_draft.which, PrintWhich::Selected);
    assert_eq!(h.state().print_draft.selected.len(), 2);
    assert_eq!(h.pager(), (1, 2));
    h.pick(None);
    h.ok();
    assert!(!h.is_open());
    let doc = pdfcraft_cos::Document::open(h.saved(0)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 2);
    // The selection gone, the remembered choice falls back to the whole document.
    h.state_mut().views[tab].select_pages(&[]);
    h.open();
    assert_eq!(h.state().print_draft.which, PrintWhich::All);
    assert!(h.state().print_draft.selected.is_empty());
}

#[test]
fn invalid_ranges_are_explained_in_the_preview() {
    let mut h = harness();
    h.open();
    // The Pages box takes typing once its choice is picked, as in the dialog's row.
    h.set("which", json!("range"));
    h.set("range", json!("7"));
    assert!(h.state().print_draft.settings(1, &[]).is_err(), "page 7 of a one-page document is out of range");
    assert!(h.preview_says_why(1), "the preview says why");
}

#[test]
fn print_is_disabled_while_the_page_range_is_invalid() {
    let mut h = harness();
    h.open();
    h.set("which", json!("range"));
    h.set("range", json!("7"));
    h.run_steps(3);
    assert!(h.get_by_role_and_label(Role::Button, "Print").accesskit_node().is_disabled(), "an out-of-range page range leaves Print disabled");
    h.set("range", json!("1"));
    h.run_steps(3);
    assert!(!h.get_by_role_and_label(Role::Button, "Print").accesskit_node().is_disabled(), "a valid range enables Print");
}

#[test]
fn cut_stack_dialog_previews_saves_and_refuses_duplex() {
    use pdfcraft_engine::print::{PageOrder, spool::Duplex};
    let mut h = harness();
    // Ten source pages, generated using the headless engine.
    let bytes = h.state().session.create_blank(200.0, 300.0, 10).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "numbered.pdf", None, bytes.as_ref().clone()).unwrap();
    h.open();
    {
        let d = &mut h.state_mut().print_draft;
        d.handling = pdfcraft_ui_egui::PrintHandling::Multiple;
        d.order = PageOrder::CutStack;
        d.per_sheet = 4;
    }
    h.run_steps(2);
    assert_eq!(h.pager(), (1, 3));
    assert!(h.has_note("note.cut_stack"));
    h.get_by_role_and_label(Role::Button, "Next").click();
    h.run_steps(2);
    assert_eq!(h.pager(), (2, 3));
    h.set("duplex", json!("long_edge"));
    assert_eq!(h.state().print_draft.duplex, Some(Duplex::LongEdge));
    assert!(h.preview_says_why(10), "cut and stack needs one-sided sheets");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err());
    h.set("duplex", json!("off"));
    h.pick(None);
    h.ok();
    assert!(!h.is_open());
    let printed = pdfcraft_cos::Document::open(h.saved(0)).unwrap();
    assert_eq!(pdfcraft_model::pages(&printed).len(), 3);
}

#[test]
fn custom_pages_per_sheet_takes_columns_by_rows() {
    use pdfcraft_engine::print::{Layout, PageOrder};
    let mut h = harness();
    let bytes = h.state().session.create_blank(200.0, 300.0, 4).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "narrow.pdf", None, bytes.as_ref().clone()).unwrap();
    h.open();
    {
        let d = &mut h.state_mut().print_draft;
        d.handling = pdfcraft_ui_egui::PrintHandling::Multiple;
        // Two by one to start.
        d.custom_grid = true;
        d.cols = 2;
        d.rows = 1;
    }
    h.run_steps(2);
    let d = &h.state().print_draft;
    let layout = d.settings(4, &[]).unwrap().layout;
    assert_eq!(
        layout,
        Layout::Multiple { cols: 2, rows: 1, per_sheet: 2, order: PageOrder::Horizontal, border: false, auto_rotate: true, custom: true }
    );
    assert_eq!(h.pager(), (1, 2));
    // A count outside 1 to 99 (set through the control channel) is explained, not drawn.
    h.state_mut().print_draft.rows = 0;
    assert!(h.state().print_draft.settings(4, &[]).is_err());
}

/// A Save as PDF whose pick or write fails leaves the dialog open with its settings, and the held OK refused.
fn refused_save_keeps_the_dialog(files: ScriptedFiles) {
    let mut h = rig_on(files, |app| {
        app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    });
    h.open();
    h.pick(None);
    let before = h.state().print_draft.clone();
    let id = h.id();
    let held = h.ok();
    assert!(matches!(held, Some(HeldOk::Refused(_))), "the refusal is held: {held:?}");
    assert!(h.state().dialogs.is_open::<HostedPrint>(), "the dialog stays open, with its settings");
    assert_eq!(h.state().dialogs.open_dialogs().last().map(|d| d.id), Some(id));
    assert_eq!(h.state().print_draft, before);
}

/// Save as PDF holds OK until the pick and the write end: a write that fails after the pick refuses
/// the held OK.
#[test]
fn a_failed_save_as_pdf_keeps_the_dialog_open() {
    // PLAN: assumed ScriptedFiles::failing_writes(path) (F-4)
    refused_save_keeps_the_dialog(ScriptedFiles::answering([Picked::Paths(vec![out(0)])]).failing_writes(out(0)));
}

/// The same when the pick itself gives no file.
#[test]
fn a_save_pick_that_gives_no_file_keeps_the_dialog_open() {
    refused_save_keeps_the_dialog(ScriptedFiles::answering([Picked::Unavailable("no picker".into())]));
}

/// #2713, #3739: Print to a CUPS queue tells it when every sheet runs landscape, so a
/// landscape sheet (an A4 landscape page, or a 6 × 4 label, laid out on Letter) is turned onto
/// the portrait paper instead of printed unrotated at 100% and cut off at the paper's edge. A
/// driver choice for the orientation stands and isn't sent twice.
#[test]
fn printing_landscape_sheets_tells_cups_to_turn_them() {
    let s = pdfcraft_engine::Session::new();
    let bytes = s.create_blank(842.0, 595.0, 2).unwrap();
    let mut h = rig_on(ScriptedFiles::new(), move |app| {
        app.open_bytes(&egui::Context::default(), "landscape.pdf", None, bytes.to_vec()).unwrap();
    });
    let jobs = record_spooler(&mut h);
    list_printers(&mut h, &["Test_Queue"]);
    // Print to the queue; `choose` sets the driver's choices once the dialog has its printer.
    let print = |h: &mut Rig, choose: &dyn Fn(&mut pdfcraft_ui_egui::PrintDraft)| -> spool::Job {
        let before = jobs.lock().unwrap().len();
        h.open();
        h.settle();
        h.pick(Some("Test_Queue"));
        h.settle();
        choose(&mut h.state_mut().print_draft);
        h.run_steps(1);
        h.ok();
        h.run_print();
        assert!(jobs.lock().unwrap().len() > before, "the job reached the spooler");
        jobs.lock().unwrap().last().cloned().unwrap()
    };
    let orientation = |job: &spool::Job| job.options.iter().filter(|(k, _)| k == "orientation-requested").map(|(_, v)| v.clone()).collect::<Vec<_>>();
    // Fit on Letter with Auto orientation lays both pages out on landscape sheets.
    let job = print(&mut h, &|_| {});
    assert_eq!(orientation(&job), if cfg!(windows) { vec![] } else { vec!["4".to_string()] }, "{:?}", job.options);
    // A driver choice for the orientation goes instead, once.
    let job = print(&mut h, &|d| {
        let choice = spool::PrinterOption {
            key: "orientation-requested".into(),
            label: "Orientation".into(),
            group: "General".into(),
            choices: vec![("4".into(), "Landscape".into()), ("5".into(), "Reverse landscape".into())],
            default: "4".into(),
        };
        d.driver_options = Some(("Test_Queue".into(), vec![choice]));
        d.driver_choices.insert("orientation-requested".into(), "5".into());
    });
    assert_eq!(orientation(&job), ["5"], "{:?}", job.options);
}

/// Properties… shows the printer driver's own options (CUPS) and sends only those changed.
#[test]
fn print_dialog_shows_the_printer_driver_options() {
    let mut h = harness();
    list_printers(&mut h, &["Test_Queue", "Other_Queue"]);
    h.open();
    h.settle();
    h.pick(None);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Properties…") && n.is_disabled()).is_some(), "Save as PDF has no driver");
    let tray = spool::PrinterOption {
        key: "InputSlot".into(),
        label: "Paper tray".into(),
        group: "Media".into(),
        choices: vec![("AutoSelect".into(), "Auto tray select".into()), ("Tray2".into(), "Tray 2".into())],
        default: "AutoSelect".into(),
    };
    h.pick(Some("Test_Queue"));
    {
        let d = &mut h.state_mut().print_draft;
        d.driver_options = Some(("Test_Queue".into(), vec![tray]));
        d.driver_choices.insert("InputSlot".into(), "Tray2".into());
        d.show_driver_options = true;
    }
    h.run_steps(2);
    assert!(h.query_all_by_label("Printer properties").next().is_some(), "the driver's own section shows");
    h.get_by_role_and_label(Role::Heading, "Media");
    h.get_by_label("Paper tray");
    // The selected sheet's media is always sent, independently of changed driver properties.
    let media = vec![("media".to_string(), "Letter".to_string())];
    assert_eq!(h.state().print_draft.job("form.pdf").options, [("InputSlot".to_string(), "Tray2".to_string()), media[0].clone()]);
    h.action("driver.reset");
    assert_eq!(h.state().print_draft.job("form.pdf").options, media, "driver defaults are not sent; selected paper is retained");
    // Options read for another printer never go with this one's job.
    h.state_mut().print_draft.driver_choices.insert("InputSlot".into(), "Tray2".into());
    h.pick(Some("Other_Queue"));
    assert_eq!(h.state().print_draft.job("form.pdf").options, media, "another queue must not inherit driver options");
}

/// The printer picked in the dialog, Save as PDF included, is still chosen the next time Print
/// opens (#3537), until a fresh list no longer has it.
#[test]
fn the_picked_printer_is_kept_for_the_next_print_while_it_is_listed() {
    let mut h = harness();
    let listed = Arc::new(Mutex::new(vec!["Office", "Front Desk"]));
    let source = listed.clone();
    h.state_mut().printer_source = Some(Arc::new(move || {
        let names = source.lock().map(|n| n.clone()).unwrap_or_default();
        names.into_iter().map(|name| printer(name, name == "Front Desk")).collect()
    }));
    let front = Destination::Printer("Front Desk".into());
    h.open();
    h.settle();
    assert_eq!(h.destination(), front, "the default first");

    // As the Printer list records a choice.
    h.pick(None);
    h.reopen();
    assert_eq!(h.destination(), Destination::File, "Save as PDF, as picked last time");

    h.pick(Some("Office"));
    h.reopen();
    assert_eq!(h.destination(), Destination::Printer("Office".into()), "Office, as picked last time");

    // Unplugged: the fresh list doesn't have it, so the dialog goes back to the default.
    *listed.lock().unwrap() = vec!["Front Desk"];
    h.reopen();
    assert_eq!(h.destination(), front);
    assert!(!h.state().print_draft.printer_picked);
}

#[test]
fn architectural_page_can_be_matched_at_100_percent() {
    let mut h = rig_on(ScriptedFiles::new(), |app| {
        let bytes = app.session.create_blank(2592.0, 1728.0, 1).unwrap();
        app.open_bytes(&egui::Context::default(), "architectural.pdf", None, bytes.as_ref().clone()).unwrap();
    });
    h.open();
    h.get_by_label("PDF page: 36.00 × 24.00 in");
    h.action("paper.match");
    h.run_steps(1);
    h.get_by_label("PDF scale: 100.00%");
    h.get_by_label("36.00 × 24.00 in");
    h.get_by_label("Printer paper and scaling may differ. Check driver settings.");
    assert_eq!(h.state().print_draft.paper_name(), "ARCH D (24 x 36 in)");
    let settings = h.state().print_draft.settings(1, &[]).unwrap();
    assert_eq!(settings.paper, (1728.0, 2592.0));
    assert_eq!(settings.layout, pdfcraft_engine::print::Layout::Size(pdfcraft_engine::print::SizeMode::Actual));
}

#[test]
fn custom_page_cannot_silently_match_a_standard_paper() {
    let mut h = harness();
    h.open();
    // The 300 x 400 pt fixture is not a standard sheet.
    assert!(pdfcraft_engine::print::matching_paper((300.0, 400.0)).is_none());
    let before = h.state().print_draft.clone();
    h.action("paper.match");
    h.run_steps(1);
    assert_eq!(h.state().print_draft.paper, before.paper);
    assert_eq!(h.state().print_draft.size, before.size);
}

/// The key of the note that points a driver without two-sided settings to its Properties.
const TWO_SIDED_NOTE: &str = "note.duplex";

/// Windows' printer list doesn't say which drivers take two-sided settings: reading a printer
/// Windows can't reach takes it about 48 s (#2192). The dialog lists every printer at once and
/// asks only about the one picked, in the background, so a printer nobody picks is never waited on.
#[test]
fn two_sided_support_is_read_only_for_the_printer_picked() {
    let mut h = harness();
    list_printers(&mut h, &["Office PCL", "Office PS", "Away WSD"]);
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    h.state_mut().two_sided_source = Some(Arc::new(move |name: &str| {
        log.lock().unwrap().push(name.to_string());
        Some(name != "Office PS")
    }));
    h.open();
    h.settle();
    assert!(h.state().print_draft.two_sided.contains_key("Office PCL"));
    assert_eq!(h.hosted().state().printers.len(), 3);
    assert!(!h.has_note(TWO_SIDED_NOTE), "a driver that prints two-sided as asked");
    h.pick(Some("Office PS"));
    h.settle();
    assert!(h.state().print_draft.two_sided.contains_key("Office PS"));
    assert!(h.has_note(TWO_SIDED_NOTE));
    assert_eq!(*asked.lock().unwrap(), ["Office PCL", "Office PS"], "each picked printer once, never the others");
}

/// A driver Windows reports without two-sided printing keeps its own setting whatever Two-sided
/// says (some print both sides with it Off), so the dialog says where to change it.
#[test]
fn a_printer_without_two_sided_printing_points_to_its_properties() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| {
        vec![
            spool::Printer { two_sided: Some(true), ..printer("Office PCL", true) },
            spool::Printer { two_sided: Some(false), ..printer("Office PS", false) },
        ]
    }));
    h.open();
    h.settle();
    assert!(!h.has_note(TWO_SIDED_NOTE), "a driver that prints two-sided as asked");
    h.pick(Some("Office PS"));
    h.settle();
    assert!(h.has_note(TWO_SIDED_NOTE));
}

/// A list that already says which drivers take two-sided settings is used as it is: no driver is
/// asked again, whichever printer is picked.
#[test]
fn a_listed_two_sided_answer_is_not_asked_again() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| {
        vec![
            spool::Printer { two_sided: Some(true), ..printer("Office PCL", true) },
            spool::Printer { two_sided: Some(false), ..printer("Office PS", false) },
        ]
    }));
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    h.state_mut().two_sided_source = Some(Arc::new(move |name: &str| {
        log.lock().unwrap().push(name.to_string());
        None
    }));
    h.open();
    h.settle();
    h.pick(Some("Office PS"));
    h.settle();
    assert!(h.has_note(TWO_SIDED_NOTE));
    assert!(asked.lock().unwrap().is_empty(), "the list's answers are used: {:?}", asked.lock().unwrap());
}

/// Reads that hang (printers Windows can't reach) don't pile up as the user picks printer after
/// printer: only as many run at once as the lane cap allows, and once one finishes the printer picked now is
/// asked; a printer picked earlier whose read found no free lane is not asked unless picked again.
#[test]
fn hanging_two_sided_reads_are_capped() {
    // The first printer says it prints two-sided and is never asked; the others are read.
    let names: Vec<String> = (0..12).map(|i| format!("Away WSD {i:02}")).collect();
    let listed = names.clone();
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(move || {
        listed.iter().enumerate().map(|(i, n)| spool::Printer { two_sided: (i == 0).then_some(true), ..printer(n, i == 0) }).collect()
    }));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| Vec::new()));
    h.state_mut().default_paper_source = Some(Arc::new(|_| None));
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    h.state_mut().two_sided_source = Some(Arc::new(move |name: &str| {
        log.lock().unwrap().push(name.to_string());
        Some(true)
    }));
    h.open();
    h.settle();
    assert!(asked.lock().unwrap().is_empty(), "the listed answer is used");
    // One pick per pass, with the host's jobs held like unreachable printers.
    for name in &names[1..] {
        h.pick(Some(name));
    }
    h.jobs.run_all();
    let first = asked.lock().unwrap().clone();
    let lanes = first.len();
    assert!(lanes > 0 && lanes < names.len() - 1, "the cap holds back some picks: {lanes} of {}", names.len() - 1);
    assert_eq!(first, names[1..=lanes], "the first picks fill the lanes, the rest find none free");
    // The lanes are free again: the printer picked now is asked, not the one picked before it.
    h.run_steps(2);
    h.jobs.run_all();
    let asked = asked.lock().unwrap().clone();
    let (last, skipped) = (&names[names.len() - 1], &names[names.len() - 2]);
    assert_eq!(asked.last(), Some(last), "the printer picked is asked once a read finishes");
    assert!(!asked.contains(skipped), "a printer picked while every lane was busy isn't asked unless picked again");
}

/// Print as image (Acrobat's Advanced Print Setup) is a choice in the Print dialog's Advanced
/// section: the saved print-ready PDF carries the page as a picture, no text.
#[test]
fn print_as_image_is_an_advanced_choice_that_saves_pictures() {
    let mut h = harness();
    h.open();
    h.state_mut().print_draft.as_image = true;
    h.run_steps(2);
    h.pick(None);
    h.ok();
    assert!(!h.is_open());
    let doc = pdfcraft_cos::Document::open(h.saved(0)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1);
    let pictures =
        doc.scan_objects().filter(|(_, o)| matches!(&**o, pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image"))).count();
    assert_eq!(pictures, 1, "the page is one picture");
}

/// The spooler answering with no printers at all says so (#160), and Save as PDF still works.
#[test]
fn an_empty_printer_list_says_none_were_found() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(Vec::new));
    h.open();
    assert!(h.hosted().state().discovering(), "the dialog doesn't wait for the printers");
    h.settle();
    assert!(!h.hosted().state().discovering());
    assert!(h.has_note("note.no_printers"));
    h.pick(None);
    assert!(h.hosted().state().ready(), "Save as PDF needs no printer");
    assert!(matches!(h.ok(), None | Some(HeldOk::Accepted)));
}

/// `PAPERS` index of the named sheet.
fn paper(name: &str) -> usize {
    pdfcraft_engine::print::PAPERS.iter().position(|p| p.0 == name).unwrap()
}

/// Label loads label stock; leaving it gives the paper the person had back (#3564).
#[test]
fn leaving_label_gives_back_the_paper_it_replaced() {
    use pdfcraft_engine::print::LABEL_PAPER;
    use pdfcraft_ui_egui::PrintHandling;
    let a4 = paper("A4");
    let mut h = harness();
    h.open();
    h.settle();
    for mode in [PrintHandling::Size, PrintHandling::Poster, PrintHandling::Multiple, PrintHandling::Booklet] {
        h.state_mut().print_draft.paper = a4;
        h.state_mut().print_draft.paper_overridden = true;
        h.state_mut().print_draft.set_handling(PrintHandling::Size);
        h.run_steps(2);
        h.state_mut().print_draft.set_handling(PrintHandling::Label);
        h.run_steps(2);
        assert_eq!(h.state().print_draft.handling, PrintHandling::Label);
        assert_eq!(h.state().print_draft.paper, LABEL_PAPER, "Label puts label stock in");
        h.state_mut().print_draft.set_handling(mode);
        h.run_steps(2);
        assert_eq!(h.state().print_draft.handling, mode);
        assert_eq!(h.state().print_draft.paper, a4, "{mode:?} gives A4 back after Label");
    }
}

/// A paper picked while in Label is the person's choice, and leaving Label keeps it.
#[test]
fn a_paper_picked_while_in_label_is_kept_when_leaving_it() {
    use pdfcraft_engine::print::LABEL_PAPER;
    use pdfcraft_ui_egui::{PrintDraft, PrintHandling};
    let (a4, letter) = (paper("A4"), paper("US Letter"));
    let mut d = PrintDraft { paper: a4, ..Default::default() };
    d.set_handling(PrintHandling::Label);
    assert_eq!(d.paper, LABEL_PAPER);
    d.paper = letter;
    d.set_handling(PrintHandling::Size);
    assert_eq!(d.paper, letter);
    // A second round trip from the kept paper gives that paper back.
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Poster);
    assert_eq!(d.paper, letter);
    // Clicking Label again while in Label doesn't forget the paper it replaced.
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Label);
    d.set_handling(PrintHandling::Multiple);
    assert_eq!(d.paper, letter);
}

/// Picking the label stock from the Paper list switches to Label (the existing behaviour); leaving
/// Label afterwards gives back the paper picked before it.
#[test]
fn label_stock_picked_from_the_paper_menu_is_undone_by_leaving_label() {
    use pdfcraft_engine::print::{LABEL_PAPER, PAPERS};
    use pdfcraft_ui_egui::PrintHandling;
    let a4 = paper("A4");
    let mut h = harness();
    h.open();
    h.state_mut().print_draft.paper = a4;
    h.run_steps(2);
    h.set("paper", json!(PAPERS[LABEL_PAPER].0));
    assert_eq!(h.state().print_draft.paper, LABEL_PAPER);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Label);
    h.state_mut().print_draft.set_handling(PrintHandling::Size);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Size);
    assert_eq!(h.state().print_draft.paper, a4);
}

/// The same through the Paper list: US Letter picked while in Label stays when Size is chosen,
/// rather than the A4 Label replaced coming back.
#[test]
fn a_paper_picked_from_the_menu_while_in_label_is_kept() {
    use pdfcraft_engine::print::{LABEL_PAPER, PAPERS};
    use pdfcraft_ui_egui::PrintHandling;
    let (a4, letter) = (paper("A4"), paper("US Letter"));
    let mut h = harness();
    h.open();
    h.settle();
    h.state_mut().print_draft.paper = a4;
    h.state_mut().print_draft.paper_overridden = true;
    h.state_mut().print_draft.set_handling(PrintHandling::Label);
    h.run_steps(3);
    assert_eq!(h.state().print_draft.paper, LABEL_PAPER);
    h.set("paper", json!(PAPERS[letter].0));
    assert_eq!(h.state().print_draft.paper, letter);
    h.state_mut().print_draft.set_handling(PrintHandling::Size);
    h.run_steps(3);
    assert_eq!(h.state().print_draft.handling, PrintHandling::Size);
    assert_eq!(h.state().print_draft.paper, letter, "the paper picked in Label is kept");
}

/// Properties… reads the printer's driver options off the GUI thread, as a job of the host: the
/// dialog keeps drawing while the read is pending, and a read that dies leaves the panel with
/// nothing rather than waiting (#1818). On Windows Properties… opens the driver's own window
/// (`HAS_PRINTER_PREFERENCES`), so there is no read to test.
#[cfg(not(any(target_arch = "wasm32", windows)))]
#[test]
fn printer_options_are_read_off_the_gui_thread_and_a_dead_read_offers_nothing() {
    use pdfcraft_engine::print::spool::PrinterOption;
    let mut h = harness();
    list_printers(&mut h, &["Office", "Dead"]);
    h.state_mut().options_source = Some(Arc::new(move |printer: &str| {
        assert_ne!(printer, "Dead", "the read died");
        vec![PrinterOption {
            key: "InputSlot".into(),
            label: "Paper tray".into(),
            group: String::new(),
            choices: vec![("Auto".into(), "Automatic".into()), ("Tray2".into(), "Tray 2".into())],
            default: "Auto".into(),
        }]
    }));
    h.open();
    h.settle();
    // Properties… for the chosen printer, with the panel closed, as a user opens it.
    let ask = |h: &mut Rig, printer: &str| {
        h.pick(Some(printer));
        h.settle();
        h.get_by_role_and_label(Role::Button, "Properties…").click();
        h.run_steps(1);
    };
    ask(&mut h, "Office");
    h.run_steps(3);
    assert_eq!(h.state().print_draft.driver_options_loading.as_deref(), Some("Office"), "the read waits for its job");
    assert!(h.state().print_draft.driver_options.is_none());
    h.get_by_label("Reading the printer's settings…");
    h.settle();
    let (read_for, options) = h.state().print_draft.driver_options.clone().expect("the options arrived");
    assert_eq!(read_for, "Office");
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].key, "InputSlot");
    h.get_by_label("Paper tray");

    h.action("driver.done");
    ask(&mut h, "Dead");
    h.settle();
    assert_eq!(h.state().print_draft.driver_options, Some(("Dead".into(), Vec::new())), "a dead read offers nothing, and the panel says so");
    h.get_by_label("This printer offers no settings of its own.");
}

/// A document whose security forbids printing (#2669) says so before the dialog opens, and the
/// top bar's Print button is disabled, instead of the dialog refusing on confirm.
#[test]
fn print_restricted_documents_say_so_before_the_dialog() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(include_bytes!("../data/form.pdf").to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b100,
        encrypt_metadata: true,
        seed: [3; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = rig_on(ScriptedFiles::new(), move |app| {
        app.open_bytes(&egui::Context::default(), "restricted.pdf", None, bytes).unwrap();
    });
    h.ctx.set_os(egui::os::OperatingSystem::Windows);
    h.run_steps(4);
    assert!(h.get_by_label("Print…").accesskit_node().is_disabled());
    h.get_by_label("Print…").hover();
    h.run_steps(30);
    h.get_by_label_contains("don't allow printing");
    assert!(!h.execute("print.dialog"));
    h.run_steps(2);
    assert!(!h.is_open());
    assert_eq!(crate::support::shown_notice(&h).as_deref(), Some("The document's security settings don't allow printing"));
}

/// With no document open the top bar's Print button is disabled too, but not for security
/// reasons: its tooltip must not blame the document's settings (#2669).
#[test]
fn print_without_a_document_does_not_blame_security() {
    let mut h = rig_on(ScriptedFiles::new(), |_| {});
    h.ctx.set_os(egui::os::OperatingSystem::Windows);
    h.run_steps(4);
    assert!(h.get_by_label("Print…").accesskit_node().is_disabled());
    h.get_by_label("Print…").hover();
    h.run_steps(30);
    assert!(h.query_by_label_contains("don't allow printing").is_none());
    assert!(!h.is_open());
}

#[test]
fn paper_choice_survives_cancel_and_settings_restore() {
    use pdfcraft_engine::print::{Orientation, PAPERS};
    let a4 = paper("A4");
    let letter = paper("US Letter");
    let mut h = harness();
    h.open();
    h.set("paper", json!(PAPERS[letter].0));
    h.set("paper", json!("A4"));
    assert_eq!(h.state().print_draft.paper, a4);
    // Paper persists; copies, pages and orientation belong to this job (grayscale is remembered, #2001).
    let draft = &mut h.state_mut().print_draft;
    draft.copies = 5;
    draft.which = pdfcraft_ui_egui::PrintWhich::Range;
    draft.range = "1".into();
    draft.orientation = Orientation::Landscape;
    draft.grayscale = true;
    let before_cancel = draft.clone();
    h.close();
    assert!(!h.is_open());
    assert_eq!(h.state().print_draft, before_cancel, "cancel keeps the existing draft behavior");
    let snapshot = h.state().settings_snapshot();
    assert_eq!(snapshot.print.print_paper.as_deref(), Some("A4"), "store the canonical name, not its position");
    assert_eq!(snapshot.print.print_paper_overridden, Some(true), "explicitly chosen paper records override flag");
    let saved = saved_json(h.state());
    let mut app = crate::control::new_app();
    restore(&mut app, &saved);
    // Grayscale is one of the preferences remembered across restarts (#2001); copies, pages and
    // orientation are this job's and are not saved.
    let expected = pdfcraft_ui_egui::PrintDraft { paper: a4, grayscale: true, paper_overridden: true, ..Default::default() };
    assert_eq!(app.print_draft, expected, "only the paper and the remembered preferences persist");

    // Default draft without override round-trips with paper_overridden remaining false.
    let default_saved = saved_json(&crate::control::new_app());
    let mut restored_default = crate::control::new_app();
    restore(&mut restored_default, &default_saved);
    assert!(!restored_default.print_draft.paper_overridden);

    // Opening print with a printer default paper selects that paper when paper_overridden is false.
    let mut default_h = harness();
    default_h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Office Printer", true)]));
    default_h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("A4".into(), (595.28, 841.89))]));
    default_h.state_mut().default_paper_source =
        Some(Arc::new(|printer: &str| if printer == "Office Printer" { Some(("A4".into(), (595.28, 841.89))) } else { None }));
    default_h.open();
    default_h.settle();
    assert_eq!(default_h.state().print_draft.paper, a4);
    assert!(!default_h.state().print_draft.paper_overridden);
    let mut restarted = harness_with_settings(Some(saved));
    restarted.open();
    assert_eq!(restarted.state().print_draft.paper, a4);
    restarted.set("paper", json!("US Letter"));
    restarted.close();
    let saved = saved_json(restarted.state());
    assert_eq!(restarted.state().settings_snapshot().print.print_paper.as_deref(), Some("US Letter"));
    let mut again = harness_with_settings(Some(saved));
    again.open();
    assert_eq!(again.state().print_draft.paper, letter);
}

#[test]
fn paper_restore_accepts_canonical_names_and_ignores_invalid_settings() {
    use pdfcraft_engine::print::PAPERS;
    for (index, (name, _)) in PAPERS.iter().enumerate() {
        let mut app = crate::control::new_app();
        restore(&mut app, &json!({"print_paper": name}).to_string());
        assert_eq!(app.print_draft.paper, index);
        assert!(!app.print_draft.paper_overridden);
        assert_eq!(app.settings_snapshot().print.print_paper.as_deref(), Some(*name));
    }
    for invalid in [
        "not JSON",
        "{}",
        r#"{"print_paper":null}"#,
        r#"{"print_paper":true}"#,
        r#"{"print_paper":[]}"#,
        r#"{"print_paper":{}}"#,
        r#"{"print_paper":"unknown"}"#,
        r#"{"print_paper":"a4"}"#,
        r#"{"print_paper":4}"#,
        r#"{"print_paper":-1}"#,
        r#"{"print_paper":4294967296}"#,
        r#"{"print_paper":18446744073709551615}"#,
        r#"{"print_paper":1e100}"#,
    ] {
        let mut app = crate::control::new_app();
        let factory = app.print_draft.paper;
        restore(&mut app, invalid);
        assert_eq!(app.print_draft.paper, factory, "{invalid}");
        restore(&mut app, r#"{"print_paper":"A4"}"#);
        let selected = app.print_draft.paper;
        restore(&mut app, invalid);
        assert_eq!(app.print_draft.paper, selected, "invalid input must not replace a valid choice: {invalid}");
    }
    let mut app = crate::control::new_app();
    app.print_draft.paper = usize::MAX;
    assert_eq!(app.settings_snapshot().print.print_paper, None);
}

#[test]
fn legacy_settings_restore_unsets_override_and_open_selects_printer_default() {
    let (letter, a4) = (paper("US Letter"), paper("A4"));
    let mut h = harness();
    let legacy = json!({ "print_paper": "US Letter" }).to_string();
    restore(h.state_mut(), &legacy);
    assert_eq!(h.state().print_draft.paper, letter);
    assert!(!h.state().print_draft.paper_overridden, "legacy settings restore leaves paper_overridden as false");

    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Office Printer", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("A4".into(), (595.28, 841.89))]));
    h.state_mut().default_paper_source =
        Some(Arc::new(|printer: &str| if printer == "Office Printer" { Some(("A4".into(), (595.28, 841.89))) } else { None }));
    h.open();
    h.settle();
    assert_eq!(h.state().print_draft.paper, a4);
    assert!(!h.state().print_draft.paper_overridden);
}

#[test]
fn auto_select_default_paper_leaves_label_stock_unchanged() {
    let mut draft = pdfcraft_ui_egui::PrintDraft {
        printer: Some("Office Printer".into()),
        handling: pdfcraft_ui_egui::PrintHandling::Label,
        paper: pdfcraft_engine::print::LABEL_PAPER,
        ..Default::default()
    };
    let a4 = paper("A4");
    let source: pdfcraft_ui_egui::DefaultPaperSource = Arc::new(move |_| Some(("A4".into(), (595.28, 841.89))));
    draft.auto_select_default_paper_with(Some(&source));
    assert_eq!(draft.paper, pdfcraft_engine::print::LABEL_PAPER);
    assert_eq!(draft.paper_before_label, Some(a4));
}

#[test]
fn auto_select_default_paper_preserves_user_orientation() {
    let mut draft = pdfcraft_ui_egui::PrintDraft {
        printer: Some("Office Printer".into()),
        orientation: pdfcraft_engine::print::Orientation::Landscape,
        ..Default::default()
    };
    let source: pdfcraft_ui_egui::DefaultPaperSource = Arc::new(move |_| Some(("A4".into(), (595.28, 841.89))));
    draft.auto_select_default_paper_with(Some(&source));
    assert_eq!(draft.orientation, pdfcraft_engine::print::Orientation::Landscape);
}

#[test]
fn cached_papers_apply_immediately_and_refresh_in_background() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Cached Printer", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(move |_| {
        calls_clone.fetch_add(1, Ordering::SeqCst);
        vec![("A4".into(), (595.28, 841.89))]
    }));
    h.state_mut().default_paper_source =
        Some(Arc::new(|printer: &str| if printer == "Cached Printer" { Some(("A4".into(), (595.28, 841.89))) } else { None }));
    h.open();
    h.settle();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let a4 = h.state().print_draft.paper;
    h.close();
    // The cached papers apply before the refresh is read.
    h.open();
    assert_eq!(h.state().print_draft.paper, a4);
    h.settle();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// #2001 with #5861: a printer keeps the paper it was last printed on (A3 here) when its driver's
/// default paper (A4) arrives from the background read afterwards; before, the default replaced
/// the remembered paper each time the dialog opened.
#[test]
fn a_printers_remembered_paper_wins_over_its_drivers_default() {
    use pdfcraft_engine::print::SizeMode;
    use pdfcraft_ui_egui::PrinterSetup;
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Office", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("A4".into(), (595.28, 841.89)), ("A3".into(), (841.89, 1190.55))]));
    h.state_mut().default_paper_source = Some(Arc::new(|_| Some(("A4".into(), (595.28, 841.89)))));
    let setup = PrinterSetup { paper: "A3".into(), size: SizeMode::Fit, custom_scale: 100.0, duplex: None };
    h.state_mut().print_draft.setups = vec![("Office".into(), setup)];
    h.open();
    h.settle();
    assert_eq!(h.destination(), Destination::Printer("Office".into()));
    assert_eq!(h.state().print_draft.paper_name(), "A3", "the remembered paper, not the driver's default");
}

#[test]
fn uncached_printer_switch_resets_driver_papers_to_standard_presets() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Label Printer", true), printer("Laser Printer", false)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|printer: &str| {
        if printer == "Label Printer" {
            vec![("Special Label 300x500".into(), (300.0, 500.0))]
        } else {
            vec![("Laser 350x550".into(), (350.0, 550.0))]
        }
    }));
    h.state_mut().default_paper_source =
        Some(Arc::new(|printer: &str| if printer == "Label Printer" { Some(("Special Label 300x500".into(), (300.0, 500.0))) } else { None }));
    h.open();
    h.settle();
    assert_eq!(h.state().print_draft.paper_name(), "Special Label 300x500");
    assert!(!h.state().print_draft.paper_overridden);

    // Switch printer via the printer change: the unoverridden custom paper resets and the old papers clear at once.
    h.pick(Some("Laser Printer"));
    h.state_mut().print_draft.reset_unoverridden_custom_paper();
    h.state_mut().print_draft.apply_printer_papers(Vec::new());
    assert_eq!(h.state().print_draft.paper_name(), "US Letter");
    assert!(
        !h.state().print_draft.papers.iter().any(|(n, _)| n == "Special Label 300x500"),
        "Custom label paper from previous printer cleared immediately"
    );
    assert!(h.state().print_draft.papers.iter().any(|(n, _)| n == "US Letter"), "Standard presets remain available");

    // Explicit override is preserved
    h.state_mut().print_draft.papers.push(("User Custom 400x600".into(), (400.0, 600.0)));
    h.state_mut().print_draft.paper = h.state().print_draft.papers.len() - 1;
    h.state_mut().print_draft.paper_overridden = true;
    h.state_mut().print_draft.reset_unoverridden_custom_paper();
    assert_eq!(h.state().print_draft.paper_name(), "User Custom 400x600");
}

#[test]
fn printer_list_refresh_resets_unoverridden_custom_paper_when_default_printer_changes() {
    let mut h = harness();
    let initial_printers = vec![printer("Label Printer", true), printer("Laser Printer", false)];
    let updated_printers = vec![printer("Laser Printer", true)];
    let calls = std::sync::atomic::AtomicUsize::new(0);
    h.state_mut().printer_source = Some(Arc::new(move || {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 { initial_printers.clone() } else { updated_printers.clone() }
    }));
    h.state_mut().printer_papers_source = Some(Arc::new(|printer: &str| {
        if printer == "Label Printer" {
            vec![("Special Label 300x500".into(), (300.0, 500.0))]
        } else {
            vec![("Laser 350x550".into(), (350.0, 550.0))]
        }
    }));
    h.state_mut().default_paper_source =
        Some(Arc::new(|printer: &str| if printer == "Label Printer" { Some(("Special Label 300x500".into(), (300.0, 500.0))) } else { None }));
    h.open();
    h.settle();
    assert_eq!(h.state().print_draft.paper_name(), "Special Label 300x500");

    // Open again: the fresh list has Laser Printer as the default.
    h.state_mut().open_print();
    h.settle();
    // Laser Printer has no default paper, so paper should be a standard preset ("US Letter"), not Printer A's custom paper
    assert_ne!(h.state().print_draft.paper_name(), "Special Label 300x500");
    assert_eq!(h.state().print_draft.paper_name(), "US Letter");
}

/// Print refuses while the printer's papers are still being read, and its answer is held as a refusal.
#[test]
fn print_button_waits_for_automatic_paper_detection_before_submission() {
    let mut h = harness();
    record_spooler(&mut h);
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Label 4x6 Printer", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("Shipping Label 4x6".into(), (288.0, 432.0))]));
    h.state_mut().default_paper_source = Some(Arc::new(|_| Some(("Shipping Label 4x6".into(), (288.0, 432.0)))));
    h.open();
    // The paper read waits as a job of the host.
    h.list();

    // While background read is pending and paper is not manually overridden:
    assert!(h.state().print_draft.papers_loading(), "papers_loading must be true while automatic detection is pending");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err(), "print_now must be refused while paper detection is pending");
    assert!(matches!(h.ok(), Some(HeldOk::Refused(_))), "the dialog keeps the refusal");

    // Overriding paper manually resolves the pending state and enables submission immediately
    h.state_mut().print_draft.paper_overridden = true;
    assert!(!h.state().print_draft.papers_loading(), "papers_loading becomes false once paper is overridden");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_ok(), "print_now succeeds when user has chosen a paper");
}

#[test]
fn gated_paper_detection_enables_print_after_worker_finishes() {
    let mut h = harness();
    record_spooler(&mut h);
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Label 4x6 Printer", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("Shipping Label 4x6".into(), (288.0, 432.0))]));
    h.state_mut().default_paper_source = Some(Arc::new(|_| Some(("Shipping Label 4x6".into(), (288.0, 432.0)))));
    h.open();
    h.list();
    assert!(h.state().print_draft.papers_loading());
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err());

    // Run the read.
    h.settle();
    assert!(!h.state().print_draft.papers_loading());
    assert_eq!(h.state().print_draft.paper_size(), (288.0, 432.0));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_ok());
}

#[test]
fn gated_paper_detection_blocks_print_on_printer_switch_reuse_until_resolved() {
    let mut h = harness();
    record_spooler(&mut h);
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Printer A", true), printer("Printer B", false)]));
    h.state_mut().printer_papers_source =
        Some(Arc::new(
            |p| {
                if p == "Printer A" { vec![("A-Paper 4x6".into(), (288.0, 432.0))] } else { vec![("B-Paper 5x7".into(), (360.0, 504.0))] }
            },
        ));
    h.state_mut().default_paper_source =
        Some(Arc::new(
            |p| {
                if p == "Printer A" { Some(("A-Paper 4x6".into(), (288.0, 432.0))) } else { Some(("B-Paper 5x7".into(), (360.0, 504.0))) }
            },
        ));

    // 1. Open print dialog: Printer A is default and starts reading papers
    h.open();
    h.list();
    assert!(h.state().print_draft.papers_loading());
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err());

    // 2. Switch to Printer B
    h.pick(Some("Printer B"));
    assert!(h.state().print_draft.papers_loading());
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err());

    // 3. Switch back to Printer A (A's read is still pending)
    h.pick(Some("Printer A"));
    assert!(h.state().print_draft.papers_loading(), "papers_loading must be restored for Printer A while its read is pending");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_err(), "print_now must be refused while Printer A's read is still pending after switch back");

    // 4. The reads run: Printer B's answer is not Printer A's paper.
    h.settle();
    assert!(!h.state().print_draft.papers_loading());
    assert_eq!(h.state().print_draft.paper_size(), (288.0, 432.0));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().print_now(&ctx_h).is_ok());
}

#[test]
fn restored_paper_keeps_preview_and_saved_sheet_dimensions() {
    use pdfcraft_engine::print::{self, PAPERS};
    let a4 = PAPERS.iter().find(|(name, _)| *name == "A4").unwrap().1;
    let mut h = harness_with_settings(Some(r#"{"print_paper":"A4","print_paper_overridden":true}"#.into()));
    h.open();
    h.get_by_label("Sheet 1 of 1");
    let id = h.state().views[h.state().active.unwrap()].id;
    let doc = h.state().session.get(id).unwrap();
    let sizes: Vec<_> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
    let labels: Vec<_> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let settings = h.state().print_draft.settings(sizes.len(), &labels).unwrap();
    assert_eq!(settings.paper, a4);
    let sheets = print::layout(&sizes, &settings).unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].size, a4, "preview uses the restored paper");
    h.pick(None);
    h.ok();
    assert!(!h.is_open());
    let printed = pdfcraft_cos::Document::open(h.saved(0)).unwrap();
    let pages = pdfcraft_model::pages(&printed);
    assert_eq!(pages.len(), sheets.len());
    let size = pages[0].display_size(&printed);
    assert!((size.0 - a4.0).abs() < 0.01 && (size.1 - a4.1).abs() < 0.01, "saved sheet: {size:?}");
}

/// A paper explicitly added while discovery is pending stays selected after its reply and reopen.
#[test]
fn adding_custom_paper_overrides_pending_driver_default() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Labeller", true)]));
    h.state_mut().printer_papers_source = Some(Arc::new(|_| vec![("Driver label".into(), (288.0, 432.0))]));
    h.state_mut().default_paper_source = Some(Arc::new(|_| Some(("Driver label".into(), (288.0, 432.0)))));
    h.open();
    // The paper read waits as a job of the host.
    h.list();
    assert!(h.state().print_draft.papers_loading());
    h.action("custom");
    let units = h.state().print_draft.units.per_point();
    h.add_custom("Card", 300.0 * units, 500.0 * units);
    assert!(h.state().print_draft.paper_overridden, "Add is an explicit paper choice");
    assert!(!h.state().print_draft.papers_loading(), "the explicit choice need not wait for detection");
    h.settle();
    assert_eq!(h.state().print_draft.paper_name(), "Card");
    let (w, h_pts) = h.state().print_draft.paper_size();
    assert!((w - 300.0).abs() < 0.01 && (h_pts - 500.0).abs() < 0.01);
    h.state_mut().open_print();
    h.settle();
    assert_eq!(h.state().print_draft.paper_name(), "Card", "reopening keeps the explicit choice");
}

/// A name entered by the user is not a keyword from the printer's media catalog.
#[test]
fn a_named_custom_paper_requests_its_dimensions_from_the_spooler() {
    let mut d = pdfcraft_ui_egui::PrintDraft::default();
    d.add_custom_paper(serde_json::from_value(json!({"name": "Card", "width": 300.0, "height": 500.0})).unwrap());
    assert_eq!(d.job("card.pdf").options, [("media".to_string(), "Custom.300x500".to_string())]);
}

/// Page setup: a paper size of one's own joins the Paper list (Custom…) and prints at its size.
/// The printer it was printed on keeps it (#2001) by name, across a restart, and Remove takes it
/// off the list; the printer's setup then leaves the paper as it is.
#[test]
fn a_custom_paper_prints_at_its_size_and_its_printer_keeps_it() {
    let mut h = harness();
    list_printers(&mut h, &["Labeller"]);
    let sent = record_spooler(&mut h);
    h.open();
    h.settle();
    assert_eq!(h.destination(), Destination::Printer("Labeller".into()));
    h.action("custom");
    // Typed in the preferences' unit, landscape: 6 × 4 inches.
    let per_point = h.state().print_draft.units.per_point();
    h.add_custom("Label 4×6", 432.0 * per_point, 288.0 * per_point);
    {
        let d = &h.state().print_draft;
        assert!(d.paper_is_custom() && d.paper_name() == "Label 4×6", "{}", d.paper_name());
        let (w, l) = d.paper_size();
        assert!((w - 288.0).abs() < 0.01 && (l - 432.0).abs() < 0.01, "portrait: {w}×{l}");
    }
    h.settle();
    h.ok();
    h.run_print();
    assert_eq!(sent.lock().unwrap().len(), 1, "the job reached the spooler");
    assert_eq!(h.state().print_draft.setups.first().map(|(p, s)| (p.as_str(), s.paper.as_str())), Some(("Labeller", "Label 4×6")));

    // After a restart the list has it, and the printer starts on it.
    let mut h = harness_restarted(h.state().settings_snapshot());
    list_printers(&mut h, &["Labeller"]);
    assert_eq!(h.state().print_draft.paper_count(), pdfcraft_engine::print::PAPERS.len() + 1);
    h.state_mut().print_draft.paper = 0;
    h.open();
    h.settle();
    assert_eq!((h.destination(), h.state().print_draft.paper_name().as_str()), (Destination::Printer("Labeller".into()), "Label 4×6"));

    // Remove takes it off the list; the printer's setup, still naming it, then changes nothing.
    h.action("paper.remove");
    let d = &mut h.state_mut().print_draft;
    assert_eq!((d.paper_count(), d.paper), (pdfcraft_engine::print::PAPERS.len(), 0));
    d.paper = 3;
    d.apply_setup();
    assert_eq!(d.paper, 3);
}

/// #2707: a booklet decides two-sided itself, as Acrobat does (its Print dialog hides Two-sided
/// while Booklet is chosen): the job flips on the short edge for both sides and prints
/// one-sided for the front or back only, and the Two-sided choice is kept for other layouts.
/// #5772: on a printer whose driver doesn't take two-sided settings, both sides leave it to the
/// driver's own setting, and the dialog says so.
#[test]
fn a_booklet_decides_two_sided_itself() {
    use pdfcraft_engine::print::{BookletSubset, spool::Duplex};
    use pdfcraft_ui_egui::PrintHandling;
    let mut h = harness();
    // Printers of its own, listed as the dialog lists them (drivers unasked, #3554), with what each
    // driver said about two-sided: the machine's would decide what a booklet asks for, and on a
    // machine with none the list that loads would take these away.
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Laser", true), printer("Inkjet", false)]));
    h.state_mut().two_sided_source = None;
    h.state_mut().print_draft.two_sided = [("Laser".to_string(), Some(true)), ("Inkjet".to_string(), Some(false))].into_iter().collect();
    h.open();
    h.settle();
    assert_eq!(h.destination(), Destination::Printer("Laser".into()));
    // Two-sided is the dialog's own setting, which the draft follows each pass.
    h.set("duplex", json!("long_edge"));
    h.state_mut().print_draft.set_handling(PrintHandling::Booklet);
    h.run_steps(2);
    let d = &h.state().print_draft;
    assert_eq!(d.duplex, Some(Duplex::LongEdge), "the Two-sided choice is kept");
    assert_eq!(d.job("x").duplex, Some(Duplex::ShortEdge), "both sides flip on the short edge");
    assert!(h.query_all_by_label("Creep").next().is_some(), "a Creep field beside Binding");
    h.state_mut().print_draft.creep = 1.5;
    let layout = h.state().print_draft.settings(4, &[]).map(|s| s.layout);
    assert!(matches!(layout, Ok(pdfcraft_engine::print::Layout::Booklet { creep, .. }) if creep == 1.5), "the creep reaches the booklet (#6777)");
    for subset in [BookletSubset::FrontOnly, BookletSubset::BackOnly] {
        h.state_mut().print_draft.booklet_subset = subset;
        assert_eq!(h.state().print_draft.job("x").duplex, Some(Duplex::Off), "{subset:?} is one pass of a manual duplex");
    }
    assert!(!h.has_note(TWO_SIDED_NOTE));
    h.pick(Some("Inkjet"));
    h.state_mut().print_draft.booklet_subset = BookletSubset::BothSides;
    h.run_steps(2);
    assert_eq!(h.state().print_draft.job("x").duplex, None, "the inkjet's own two-sided setting prints both sides");
    assert!(h.has_note(TWO_SIDED_NOTE), "the dialog says so");
    h.state_mut().print_draft.booklet_subset = BookletSubset::FrontOnly;
    assert_eq!(h.state().print_draft.job("x").duplex, Some(Duplex::Off), "one side stays one-sided");
    h.pick(Some("Laser"));
    h.state_mut().print_draft.set_handling(PrintHandling::Size);
    h.run_steps(2);
    assert_eq!(h.state().print_draft.job("x").duplex, Some(Duplex::LongEdge), "back to the choice");
}

/// #5861: The print dialog supports all papers detected from the printer driver, matches custom
/// page sizes against them, and imposes on the chosen dimensions.
#[test]
fn printer_papers_and_properties_selection() {
    let mut h = harness();
    h.open();

    // Initial state offers standard PAPERS.
    assert!(h.state().print_draft.papers.len() >= 21);
    assert_eq!(h.state().print_draft.paper_name(), "US Letter");

    // When the driver offers additional custom papers, they are available in d.papers.
    let custom_paper = ("Custom 333x444".to_string(), (333.0, 444.0));
    h.state_mut().print_draft.papers.push(custom_paper.clone());
    let custom_index = h.state().print_draft.papers.len() - 1;

    h.state_mut().print_draft.paper = custom_index;
    assert_eq!(h.state().print_draft.paper_name(), "Custom 333x444");
    assert_eq!(h.state().print_draft.paper_size(), (333.0, 444.0));

    // Matching page at 100% finds the custom paper
    assert_eq!(h.state().print_draft.matching_paper((333.0, 444.0)), Some(custom_index));

    // Imposed settings use the exact dimensions of the detected paper
    let settings = h.state().print_draft.settings(1, &["1".into()]).unwrap();
    assert_eq!(settings.paper, (333.0, 444.0));

    // Applying driver paper adapts paper selection and orientation
    h.state_mut().print_draft.apply_driver_paper("4 x 6 in label (101.6 x 152.4 mm)".into(), (288.0, 432.0));
    assert_eq!(h.state().print_draft.paper, pdfcraft_engine::print::LABEL_PAPER);
    assert_eq!(h.state().print_draft.orientation, pdfcraft_engine::print::Orientation::Portrait);

    // Landscape driver media adapts orientation to Landscape
    h.state_mut().print_draft.apply_driver_paper("Landscape Custom".into(), (500.0, 300.0));
    assert_eq!(h.state().print_draft.orientation, pdfcraft_engine::print::Orientation::Landscape);
    assert_eq!(h.state().print_draft.paper_name(), "Landscape Custom");
}

/// Creep the field allows can still move a thick booklet's innermost pages too far; the preview
/// says to reduce it rather than showing no sheets without a reason (#6777).
#[test]
fn too_much_creep_for_a_thick_booklet_is_explained_in_the_preview() {
    let mut h = rig_on(ScriptedFiles::new(), |app| {
        app.open_bytes(&egui::Context::default(), "thick.pdf", None, crate::support::fixtures::pdf_pages(400)).unwrap();
    });
    h.open();
    h.state_mut().print_draft.set_handling(pdfcraft_ui_egui::PrintHandling::Booklet);
    h.run_steps(2);
    h.state_mut().print_draft.creep = 5.0;
    h.run_steps(2);
    h.get_by_label_contains("Reduce Creep");
    h.state_mut().print_draft.creep = 0.5;
    h.run_steps(2);
    assert!(h.query_by_label_contains("Reduce Creep").is_none(), "a creep that fits lays the booklet out");
}

/// An app restored from `saved` settings (if any), with a form open and its Print dialog shown.
fn print_opened_with(saved: Option<&str>) -> PdfCraftApp {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    if let Some(saved) = saved {
        restore(&mut app, saved);
    }
    app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    app.open_print();
    app
}

/// The choices that describe how an office prints (two-sided, grayscale, collate, page sizing,
/// comments & forms, a booklet's binding side, Print as image and its resolution) survive a
/// restart; the ones that describe one job (copies, pages, layout, orientation) start from the
/// defaults each time Print opens, in the same session too (ISO 32000-2 Table 147 initialises them
/// per document) (#2001, #6392).
#[test]
fn print_preferences_survive_a_restart_but_the_jobs_own_choices_reset() {
    use pdfcraft_engine::print::{Binding, BookletSubset, Content, LABEL_PAPER, Orientation, PageOrder, SizeMode, Subset, spool::Duplex};
    use pdfcraft_ui_egui::{PrintHandling, PrintWhich};
    let open = print_opened_with;
    let mut app = open(None);
    let d = &mut app.print_draft;
    d.duplex = Some(Duplex::LongEdge);
    d.grayscale = true;
    d.collate = false;
    d.size = SizeMode::Custom(62.5);
    d.custom_scale = 62.5;
    d.content = Content::Document;
    d.binding = Binding::Right;
    d.as_image = true;
    d.dpi = 600.0;
    d.copies = 5;
    d.which = PrintWhich::Range;
    d.range = "2-3".into();
    d.subset = Subset::Even;
    d.reverse = true;
    d.handling = PrintHandling::Booklet;
    d.booklet_subset = BookletSubset::FrontOnly;
    d.per_sheet = 16;
    (d.custom_grid, d.cols, d.rows) = (true, 3, 5);
    d.order = PageOrder::CutStack;
    d.orientation = Orientation::Landscape;
    let saved = saved_json(&app);

    // Opened again in the same session: this job's choices, the layout's own options included,
    // start over; the rest carry on.
    app.open_print();
    let d = &app.print_draft;
    assert_eq!((d.copies, d.which, d.range.as_str(), d.subset, d.reverse), (1, PrintWhich::All, "", Subset::All, false));
    assert_eq!((d.handling, d.orientation), (PrintHandling::Size, Orientation::Auto));
    assert_eq!((d.booklet_subset, d.per_sheet, d.order), (BookletSubset::BothSides, 2, PageOrder::Horizontal));
    let fresh = pdfcraft_ui_egui::PrintDraft::default();
    assert_eq!((d.custom_grid, d.cols, d.rows), (false, fresh.cols, fresh.rows));
    assert_eq!((d.duplex, d.grayscale, d.collate, d.size, d.binding), (Some(Duplex::LongEdge), true, false, SizeMode::Custom(62.5), Binding::Right));

    // A job printed on label stock: the next one is back on the paper Label replaced (#3564).
    let a4 = paper("A4");
    app.print_draft.paper = a4;
    app.print_draft.set_handling(PrintHandling::Label);
    assert_eq!(app.print_draft.paper, LABEL_PAPER);
    app.open_print();
    assert_eq!((app.print_draft.handling, app.print_draft.paper), (PrintHandling::Size, a4));

    let again = open(Some(&saved));
    let d = &again.print_draft;
    assert_eq!(d.duplex, Some(Duplex::LongEdge));
    assert!(d.grayscale);
    assert!(!d.collate);
    assert_eq!(d.size, SizeMode::Custom(62.5));
    assert_eq!(d.custom_scale, 62.5);
    assert_eq!(d.content, Content::Document);
    assert_eq!(d.binding, Binding::Right, "a booklet's binding side is kept (#2707)");
    assert!(d.as_image, "Print as image is kept, for a printer that needs pictures (#6392)");
    assert_eq!(d.dpi, 600.0, "and its resolution");
    assert_eq!(d.copies, 1, "copies are this job's");
    assert_eq!(d.which, PrintWhich::All, "the page choice is this job's");
    assert_eq!(d.range, "");
}

/// Settings are untrusted: an unknown name, a scale outside the dialog's range or a resolution it
/// doesn't offer keeps the default (#2001, #6392).
#[test]
fn print_preferences_that_dont_parse_keep_the_defaults() {
    use pdfcraft_engine::print::{Binding, Content, SizeMode, spool::Duplex};
    let open = print_opened_with;
    let garbage = r#"{"print": {"duplex": "sideways", "grayscale": "yes", "collate": null, "size": "custom", "custom_scale": 1e9, "comments_forms": 7, "binding": "up", "as_image": 1, "dpi": 72}}"#;
    let d = open(Some(garbage)).print_draft;
    assert!(!d.as_image);
    assert_eq!(d.dpi, 300.0, "a resolution the dialog doesn't offer keeps the default");
    assert_eq!(d.duplex, None, "an unknown name keeps the printer's own default");
    assert!(!d.grayscale);
    assert!(d.collate);
    assert_eq!(d.custom_scale, 100.0, "a scale outside 1..=1000 keeps the default");
    assert_eq!(d.size, SizeMode::Custom(100.0), "custom is honoured with the default scale");
    assert_eq!(d.content, Content::DocumentAndMarkups);
    assert_eq!(d.binding, Binding::Left);
    let d = open(Some(r#"{"print": 5}"#)).print_draft;
    assert_eq!(d.size, SizeMode::Fit);
    // Leaving two-sided to the printer is remembered as such, over a different earlier choice.
    let d = open(Some(r#"{"print": {"duplex": "printer"}}"#)).print_draft;
    assert_eq!(d.duplex, None);
    let d = open(Some(r#"{"print": {"duplex": "off"}}"#)).print_draft;
    assert_eq!(d.duplex, Some(Duplex::Off));
}

/// #2001: each printer keeps the paper, sizing and two-sided choice it last printed with, across
/// a restart, and the printer printed to last is chosen again. A label printer keeps 4 × 6 in
/// at Actual size, single-sided, while the office printer keeps A4 at Fit, two-sided.
#[test]
fn each_printer_keeps_what_it_last_printed_with_across_a_restart() {
    use pdfcraft_engine::print::spool::{Duplex, Job};
    use pdfcraft_engine::print::{LABEL_PAPER, SizeMode};
    let a4 = paper("A4");
    let listed = Arc::new(Mutex::new(vec!["Office", "D520BT_Z"]));
    // The printers and the spooler of a rig; the jobs sent are shared across the restarts.
    let sent: Arc<Mutex<Vec<Job>>> = Arc::default();
    let attach = |h: &mut Rig| {
        let source = listed.clone();
        h.state_mut().printer_source = Some(Arc::new(move || {
            let names = source.lock().map(|n| n.clone()).unwrap_or_default();
            names.into_iter().map(|name| printer(name, name == "Office")).collect()
        }));
        h.state_mut().printer_papers_source = Some(Arc::new(|_| Vec::new()));
        h.state_mut().default_paper_source = Some(Arc::new(|_| None));
        let log = sent.clone();
        h.state_mut().spooler = Some(Arc::new(move |_pdf: &[u8], job: &Job| {
            log.lock().unwrap().push(job.clone());
            Ok(String::new())
        }));
    };
    let print = |h: &mut Rig| -> Job {
        let before = sent.lock().unwrap().len();
        h.settle();
        h.ok();
        h.run_print();
        assert!(sent.lock().unwrap().len() > before, "the job reached the spooler");
        sent.lock().unwrap().last().cloned().unwrap()
    };
    let setup = |h: &Rig| {
        let d = &h.state().print_draft;
        (h.destination(), d.paper, d.size, d.duplex)
    };
    let printer_dest = |name: &str| Destination::Printer(name.into());
    let label = (printer_dest("D520BT_Z"), LABEL_PAPER, SizeMode::Actual, Some(Duplex::Off));
    let office = (printer_dest("Office"), a4, SizeMode::Fit, Some(Duplex::LongEdge));

    let mut h = harness();
    attach(&mut h);
    h.open();
    h.settle();
    assert_eq!(h.destination(), printer_dest("Office"), "the system default first");
    h.pick(Some("D520BT_Z"));
    {
        let d = &mut h.state_mut().print_draft;
        d.paper = LABEL_PAPER;
        d.size = SizeMode::Actual;
    }
    h.set("duplex", json!("off"));
    assert_eq!(print(&mut h).printer.as_deref(), Some("D520BT_Z"));
    h.reopen();
    h.pick(Some("Office"));
    {
        let d = &mut h.state_mut().print_draft;
        d.paper = a4;
        d.size = SizeMode::Fit;
    }
    h.set("duplex", json!("long_edge"));
    assert_eq!(print(&mut h).printer.as_deref(), Some("Office"));

    // Choosing the label printer in the dialog brings back what it printed with.
    h.reopen();
    assert_eq!(setup(&h), office, "the same printer keeps this session's choices");
    h.pick(Some("D520BT_Z"));
    h.settle();
    assert_eq!(setup(&h), label);
    let job = print(&mut h);
    assert_eq!(job.duplex, Some(Duplex::Off));

    // After a restart the label printer, printed to last, is chosen again with its setup, not
    // the system default; choosing the office printer brings back A4, Fit and two-sided.
    let saved = h.state().settings_snapshot();
    let mut r = harness_restarted(saved.clone());
    attach(&mut r);
    r.open();
    r.settle();
    assert_eq!(setup(&r), label, "the label printer and its 4 × 6 in at Actual size, single-sided");
    r.pick(Some("Office"));
    r.settle();
    assert_eq!(setup(&r), office);

    // A remembered printer that is gone falls back to the system default, with its own setup.
    *listed.lock().unwrap() = vec!["Office"];
    let mut gone = harness_restarted(saved);
    attach(&mut gone);
    gone.open();
    gone.settle();
    assert_eq!(setup(&gone), office);
}
