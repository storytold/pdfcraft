//! The UI control channel (M3.9): an agent can see the widget tree, click, type, press keys,
//! run commands, change view options and take screenshots of the running app.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use crate::support::AppHarness as _;
use craft::automation::{ControlRequest, ControlResponse};
use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles, Wake};
use craft::platform::open::FileIntake;
use craft::platform::watch::FileWatch;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::settings::import_json;
use pdfcraft_ui_egui::{AppServices, PdfCraftApp};
use serde_json::{Value, json};

/// Where a test sends its requests: the other end is attached to the app with `attach_control`.
pub(crate) type ControlClient = Sender<ControlRequest>;

/// What a request came to: the result of the method, or the error it refused with.
pub(crate) type Reply = Result<Value, String>;

/// The app's identity for a test host.
pub(crate) fn info() -> AppInfo {
    AppInfo { id: "pdfcraft", name: "PdfCraft", version: "test" }
}

/// The services a test gives the app: no wake-ups, no autosave folder, the real GitHub check source,
/// a fresh file intake and an inline watch.
pub(crate) fn services() -> AppServices {
    services_asking(None)
}

/// `services()` whose update check asks `update_source` in place of GitHub.
pub(crate) fn services_asking(update_source: Option<craft::update::updater::CheckSource>) -> AppServices {
    AppServices::new(
        Wake::none(),
        craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new()),
        None,
        update_source,
        FileIntake::new(Wake::none()).0,
        FileWatch::new(Wake::none()).inline(),
    )
}

/// An app on `host`.
pub(crate) fn app_on(host: Host) -> PdfCraftApp {
    PdfCraftApp::with_host(Arc::new(host), services())
}

/// An app on the default host: threads run its jobs and every picker answers Cancelled.
pub(crate) fn new_app() -> PdfCraftApp {
    app_on(Host::builder(info()).build())
}

/// An app whose pickers answer from `files`.
pub(crate) fn app_with_files(files: Arc<ScriptedFiles>) -> PdfCraftApp {
    app_on(Host::builder(info()).files(files).build())
}

/// An app whose background jobs wait until the test runs them (`jobs.run_all()`), so a request that
/// starts one is answered `pending` until then.
pub(crate) fn app_with_manual_jobs() -> (PdfCraftApp, Arc<ManualJobs>) {
    let jobs = Arc::new(ManualJobs::new());
    (app_on(Host::builder(info()).jobs(jobs.clone()).build()), jobs)
}

/// Sends a request without waiting; its answer arrives on the returned receiver once a frame has
/// handled it.
pub(crate) fn send(c: &ControlClient, method: &str, params: Value) -> Receiver<ControlResponse> {
    let (request, rx) = ControlRequest::new(method, params);
    c.send(request).unwrap_or_else(|_| panic!("{method}: the app no longer reads the control channel"));
    rx
}

/// The `{ok, result | error}` answer as a `Result`; the answer must have arrived.
pub(crate) fn answer(rx: &Receiver<ControlResponse>) -> Reply {
    reply(&rx.try_recv().expect("the request was answered by the frame"))
}

/// The text each page shows, in order, as the app's session holds it.
fn page_texts(app: &PdfCraftApp) -> Vec<String> {
    let doc = app.session.get(app.views[0].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
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

fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_pages(5)
}

fn harness_pages(pages: usize) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_pages_at(pages, 1400.0, 1.0)
}

fn harness_pages_at(pages: usize, width: f32, scale: f32) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_on(new_app(), pages, width, scale)
}

/// `harness_pages` on a file service that answers from `files`.
fn harness_with_files(pages: usize, files: Arc<ScriptedFiles>) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_on(app_with_files(files), pages, 1400.0, 1.0)
}

/// `app` with a `pages`-page document open, in a window `width` wide at `scale`, and the control
/// channel attached.
fn harness_on(app: PdfCraftApp, pages: usize, width: f32, scale: f32) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(width, 900.0)).with_pixels_per_point(scale).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        // What `automation::server::serve` does for the running app: keep the presented frame's access tree for
        // `ui.inspect`, `ui.click {label}` and `ui.focus`.
        craft::ui::runtime::windows::WindowHost::of(&cc.egui_ctx).keep_access_tree();
        {
            let mut app = app;
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, fixture(pages)).unwrap();
            app
        }
    });
    h.run_steps(4);
    (h, tx)
}

/// Send a request and run frames until it is answered.
pub(crate) fn call(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Reply {
    let rx = send(c, method, params);
    let mut response = None;
    let answered = crate::support::step_until(h, |_| {
        response = rx.try_recv().ok();
        response.is_some()
    });
    match response {
        Some(response) if answered => reply(&response),
        _ => panic!("{method}: no reply in 30 s"),
    }
}

/// A response as a `Result`: the method's result, or the error it refused with.
fn reply(response: &ControlResponse) -> Reply {
    if response["ok"] == true { Ok(response["result"].clone()) } else { Err(response["error"].as_str().unwrap_or_default().to_owned()) }
}

pub(crate) fn ok(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Value {
    call(h, c, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Presses `from`, moves through `steps` points to `to` and releases, the way `ui.drag` did: a
/// click that holds, moves, then releases. `modifiers` are held for the whole gesture and `button`
/// is `"left"`, `"secondary"` or `"middle"`.
fn drag(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, from: Value, to: Value, steps: usize, modifiers: &[&str], button: &str) {
    let modifiers_held = |names: &[&str]| -> Value { Value::Object(names.iter().map(|name| ((*name).to_owned(), Value::Bool(true))).collect()) };
    let held = modifiers_held(modifiers);
    ok(h, c, "ui.click", json!({"x": from[0], "y": from[1], "hold": true, "button": button, "modifiers": held}));
    let point = |p: &Value| [p[0].as_f64().unwrap_or_default(), p[1].as_f64().unwrap_or_default()];
    let (from, to) = (point(&from), point(&to));
    let steps = steps.max(1);
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        ok(h, c, "ui.move", json!({"x": from[0] + (to[0] - from[0]) * t, "y": from[1] + (to[1] - from[1]) * t, "modifiers": held}));
    }
    // `ui.release` takes no modifiers: releasing the button ends the gesture they were held for.
    ok(h, c, "ui.release", json!({"button": button}));
    if !modifiers.is_empty() {
        ok(h, c, "ui.release", json!({}));
    }
}

/// Let the next real frames run after a control setting that changes the interface fonts or
/// opens a modal. `ui.set` answers in the input pass, and a font change discards the pass that
/// applies it, so the modal's labels are not in the accessibility tree until those frames have
/// been laid out.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    h.run_steps(3);
}

/// Sends a request and runs the host's held-back jobs with each frame until it is answered, for a
/// request whose answer waits on a job.
fn call_running(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, jobs: &ManualJobs, method: &str, params: Value) -> Reply {
    let rx = send(c, method, params);
    for _ in 0..30 {
        jobs.run_all();
        h.step();
        if let Ok(response) = rx.try_recv() {
            return reply(&response);
        }
    }
    panic!("{method}: no reply after 30 frames");
}

/// Runs the jobs the host holds back (`ManualJobs`) and the frames that apply their results; a job
/// that starts another is run by the next round.
fn run_jobs(h: &mut Harness<'static, PdfCraftApp>, jobs: &ManualJobs) {
    for _ in 0..4 {
        jobs.run_all();
        h.step();
    }
}

#[test]
fn comment_pin_is_reported_and_persistent() {
    let (mut h, c) = harness();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["comment_tool_pinned"], false);
    for pinned in [true, false] {
        ok(&mut h, &c, "ui.set", json!({"key":"comment-tool-pinned","value":pinned}));
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["comment_tool_pinned"], pinned);
        // A fresh app that starts from what this one would save holds the same pin, whatever it held.
        let (mut restarted, rc) = harness_pages(1);
        ok(&mut restarted, &rc, "ui.set", json!({"key":"comment-tool-pinned","value":!pinned}));
        restarted.state_mut().apply_settings(&h.state().settings_snapshot());
        restarted.run_steps(2);
        assert_eq!(ok(&mut restarted, &rc, "ui.state", json!({}))["comment_tool_pinned"], pinned);
        for invalid in [json!("yes"), json!(1), json!(null), json!([]), json!({})] {
            assert!(call(&mut h, &c, "ui.set", json!({"key":"comment-tool-pinned","value":invalid})).is_err());
            assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["comment_tool_pinned"], pinned);
        }
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["dirty"], false);
    }
}

/// What `ui.state` reports at `path`.
fn reported(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, path: &[&str]) -> Value {
    path.iter().fold(ok(h, c, "ui.state", json!({})), |value, key| value[*key].clone())
}

/// The app's agent contract: each `ui.set` key changes the `ui.state` field it names, a value it
/// refuses changes nothing, and none of it dirties the document.
#[test]
fn ui_set_keys_change_what_ui_state_reports() {
    let (mut h, c) = harness();
    let s = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(s["documents"][0]["name"], "doc.pdf");
    assert_eq!(s["documents"][0]["pages"], 5);
    assert_eq!(s["active"]["page"], 1);
    assert_eq!(s["active"]["page_errors"], json!([]));
    assert_eq!(s["active"]["selected_pages"], json!([]));
    assert_eq!(s["active"]["grid_zoom"], 1.0);
    ok(&mut h, &c, "ui.set", json!({"key": "panel", "value": "pages"}));
    // The key, a value it takes, the field that changes and what it holds, and a value it refuses.
    let rows: [(&str, Value, &[&str], Value, Value); 6] = [
        ("thumbnail-zoom", json!("50"), &["active", "thumbnail_zoom"], json!(0.5), json!("NaN")),
        ("page", json!(4), &["active", "page"], json!(4), json!("not a page")),
        ("panel", json!("bookmarks"), &["side_panels", "side_panel"], json!("Bookmarks"), json!("nonsense")),
        ("select", json!("2,4"), &["active", "selected_pages"], json!([2, 4]), json!("0,6")),
        ("select", json!("4-2,3"), &["active", "selected_pages"], json!([2, 3, 4]), json!("0,6")),
        ("select", json!(""), &["active", "selected_pages"], json!([]), json!("0,6")),
    ];
    for (key, value, path, expected, refused) in rows {
        ok(&mut h, &c, "ui.set", json!({"key": key, "value": value}));
        h.run_steps(3);
        assert_eq!(reported(&mut h, &c, path), expected, "ui.set {key} {value}");
        assert!(call(&mut h, &c, "ui.set", json!({"key": key, "value": refused})).is_err(), "ui.set {key} {refused} is refused");
        assert_eq!(reported(&mut h, &c, path), expected, "a refused {key} changes nothing");
    }
    assert_eq!(reported(&mut h, &c, &["documents"])[0]["dirty"], false);
    assert_eq!(reported(&mut h, &c, &["active", "grid_zoom"]), 1.0, "the thumbnail zoom is not the Organize grid's");
}

#[test]
fn page_shadows_are_an_observable_app_wide_preference() {
    let (mut h, c) = harness();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["show_page_edge_shadows"], true);
    for value in ["false", "true"] {
        ok(&mut h, &c, "ui.set", json!({ "key": "show_page_edge_shadows", "value": value }));
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["show_page_edge_shadows"], value == "true");
    }
    for value in ["off", "1", "False", ""] {
        assert!(call(&mut h, &c, "ui.set", json!({ "key": "show_page_edge_shadows", "value": value })).is_err());
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["show_page_edge_shadows"], true);
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "show_page_edge_shadows", "value": false }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["show_page_edge_shadows"], false);
    h.state_mut().active = None;
    ok(&mut h, &c, "ui.set", json!({ "key": "show_page_edge_shadows", "value": "false" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["show_page_edge_shadows"], false);
}

#[test]
fn control_button_focus_preserves_document_undo_redo_shortcuts() {
    let (mut h, c) = harness_pages(2);
    ok(&mut h, &c, "ui.focus", json!({"label": "Convert"}));
    ok(&mut h, &c, "ui.key", json!({"key": "Enter"}));
    let tabs = ok(&mut h, &c, "ui.inspect", json!({"query": "All tools", "role": "Tab"}));
    let id = tabs["widgets"][0]["id"].as_str().expect("the All tools mode tab");
    ok(&mut h, &c, "ui.click", json!({"id": id}));
    assert!(h.ctx.egui_wants_keyboard_input());
    assert!(!h.ctx.text_edit_focused());
    ok(&mut h, &c, "ui.command", json!({"id": "page.insert_blank"}));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["pages"], 3);
    ok(&mut h, &c, "ui.key", json!({"key": "Z", "modifiers": {"command": true}}));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["pages"], 2);
    ok(&mut h, &c, "ui.key", json!({"key": "Z", "modifiers": {"command": true, "shift": true}}));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["pages"], 3);
}

#[test]
fn control_text_focus_retains_text_undo_ownership() {
    let (mut h, c) = harness_pages(2);
    let ctx_h = h.ctx.clone();
    h.state_mut()
        .apply_edit(&ctx_h, pdfcraft_engine::Edit::AddBookmark { parent: vec![], index: 0, title: "Target".into(), page: 0, left: None, top: None });
    ok(&mut h, &c, "ui.command", json!({"id": "page.insert_blank"}));
    ok(&mut h, &c, "ui.set", json!({"key": "panel", "value": "bookmarks"}));
    let widgets = ok(&mut h, &c, "ui.inspect", json!({"query": "Search", "role": "TextInput"}));
    ok(&mut h, &c, "ui.focus", json!({"id": widgets["widgets"][0]["id"]}));
    assert!(h.ctx.text_edit_focused());
    ok(&mut h, &c, "ui.type", json!({"text": "target"}));
    assert_eq!(search_text(&h), "target");
    ok(&mut h, &c, "ui.key", json!({"key": "Z", "modifiers": {"command": true}}));
    assert_eq!(search_text(&h), "");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["pages"], 3);
}

/// What the Bookmarks panel's Search field holds, as the access tree reports it.
fn search_text(h: &Harness<'static, PdfCraftApp>) -> String {
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search").value().unwrap_or_default()
}

/// Whether the command palette is on screen: its command rows are list-box options in the access tree.
fn palette_on_screen(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.query_all_by_role(egui::accesskit::Role::ListBoxOption).next().is_some()
}

fn close_requested(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)))
}

#[test]
fn control_quit_preserves_dirty_documents_and_cancel_keeps_them_open() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.command", json!({"id": "page.insert_blank"}));
    for params in [json!({"force": true}), json!({"discard": true}), json!([])] {
        assert!(call(&mut h, &c, "ui.quit", params).is_err());
        assert!(!close_requested(&h));
    }
    assert_eq!(ok(&mut h, &c, "ui.quit", json!({})), json!({"quitting": false, "needs_confirmation": true}));
    assert!(!close_requested(&h));
    assert_eq!(ok(&mut h, &c, "ui.quit", Value::Null), json!({"quitting": false, "needs_confirmation": true}));
    assert!(!close_requested(&h));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["close_prompt"], true);
    assert_eq!(state["documents"][0]["dirty"], true);
    assert_eq!(state["documents"][0]["pages"], 6);
    ok(&mut h, &c, "ui.click", json!({"label": "Cancel"}));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["close_prompt"], false);
    assert_eq!(state["documents"].as_array().unwrap().len(), 1);
    assert_eq!(state["documents"][0]["dirty"], true);
    assert_eq!(state["documents"][0]["pages"], 6);
    assert!(!close_requested(&h));
}

#[test]
fn bookmark_titles_can_be_searched_over_control() {
    let (mut h, c) = harness();
    for (index, title, page) in [(0, "Background", 0), (1, "Target chapter", 3)] {
        let ctx_h = h.ctx.clone();
        h.state_mut()
            .apply_edit(&ctx_h, pdfcraft_engine::Edit::AddBookmark { parent: vec![], index, title: title.into(), page, left: None, top: None });
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "panel", "value": "bookmarks" }));
    h.run_steps(3);
    let widgets = ok(&mut h, &c, "ui.inspect", json!({ "query": "Search", "role": "TextInput" }));
    let rect = &widgets["widgets"][0]["rect"];
    let x = (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.0;
    let y = (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.0;
    ok(&mut h, &c, "ui.click", json!({ "x": x, "y": y }));
    ok(&mut h, &c, "ui.type", json!({ "text": "target" }));
    assert_eq!(search_text(&h), "target", "the Search field holds what was typed");
    h.get_by_label("Target chapter");
    assert!(h.query_by_label("Background").is_none());
    ok(&mut h, &c, "ui.click", json!({ "label": "Target chapter" }));
    assert_eq!(h.state().views[0].current(), 3);
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/control-filtered-bookmarks.png")).unwrap();
    }
    ok(&mut h, &c, "ui.click", json!({ "label": "Clear" }));
    assert_eq!(search_text(&h), "", "Clear empties the Search field");
    h.get_by_label("Background");
    h.get_by_label("Target chapter");
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/control-cleared-bookmarks.png")).unwrap();
    }
}

#[test]
fn theme_commands_and_options_report_preference_and_effective_colours() {
    let (mut h, c) = harness();
    // The desktop reports its scheme as the windowing integration does: the raw field and an event.
    let desktop = |h: &mut Harness<'static, PdfCraftApp>, theme: egui::Theme| {
        h.input_mut().system_theme = Some(theme);
        h.input_mut().events.push(egui::Event::SystemTheme(theme));
    };
    desktop(&mut h, egui::Theme::Dark);
    h.run_steps(2);
    for (id, preference, effective) in
        [("view.theme.system", "System", "Dark"), ("view.theme.light", "Light", "Light"), ("view.theme.dark", "Dark", "Dark")]
    {
        ok(&mut h, &c, "ui.command", json!({ "id": id }));
        let state = ok(&mut h, &c, "ui.state", json!({}));
        assert_eq!(state["theme_preference"], preference);
        assert_eq!(state["theme"], effective);
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "theme", "value": "system" }));
    desktop(&mut h, egui::Theme::Light);
    h.run_steps(2);
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["theme_preference"], "System");
    assert_eq!(state["theme"], "Light");
    assert!(call(&mut h, &c, "ui.set", json!({ "key": "theme", "value": "purple" })).is_err());
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["theme_preference"], "System");
}

#[test]
fn lithuanian_inspector_title_updates_through_the_control_channel() {
    if crate::support::skip_without_catalogs("lithuanian_inspector_title_updates_through_the_control_channel") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "panel", "value": "comments" }));
    h.run_steps(2);
    h.get_by_label("Inspector");
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "lt" }));
    h.run_steps(2);
    h.get_by_label("Inspektorius");
    assert!(h.query_by_label("Inspector").is_none());
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"], documents);
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "en" }));
    h.run_steps(2);
    h.get_by_label("Inspector");
}

/// Switching the interface language changes labels only: documents, their dirty state and the
/// command ids agents drive stay exactly the same.
#[test]
fn language_switch_preserves_document_and_command_ids() {
    if crate::support::skip_without_catalogs("language_switch_preserves_document_and_command_ids") {
        return;
    }
    use pdfcraft_ui_egui::i18n;
    let (mut h, c) = harness();
    let doc = h.state().views[0].id;
    h.state_mut().session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    assert_eq!(documents[0]["dirty"], true);
    let commands = ok(&mut h, &c, "ui.commands", json!({}));
    for code in ["ja", "zh-hans", "fr", "de", "uk", "lt", "en"] {
        if crate::support::skip_without_glyphs(&format!("language_switch_preserves_document_and_command_ids ({code})"), code, "") {
            continue;
        }
        ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": code }));
        h.run_steps(2);
        let state = ok(&mut h, &c, "ui.state", json!({}));
        assert_eq!(state["language"], code);
        assert_eq!(state["documents"], documents);
        assert_eq!(ok(&mut h, &c, "ui.commands", json!({})), commands);

        let lang = i18n::Lang::from_code(code).unwrap();
        // A title without an ASCII letter shows its mnemonic after it ("ファイル(F)").
        let file = i18n::tr(lang, "File");
        h.get_by(|n| n.role() == egui::accesskit::Role::MenuItem && n.label().is_some_and(|l| l.starts_with(file))).click();
        h.run_steps(2);
        // The mnemonic goes before an ellipsis: "開く(O)…".
        let open = i18n::tr(lang, "Open…").trim_end_matches('…');
        let open_row = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == egui::accesskit::Role::MenuItem && n.label().is_some_and(|l| l.starts_with(open))
        };
        assert!(h.query_all_by(open_row).next().is_some(), "File lists Open… in {code}");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        h.run_steps(2);
    }
    let error = call(&mut h, &c, "ui.set", json!({ "key": "language", "value": "xx" })).unwrap_err();
    let codes = std::iter::once(i18n::AUTO).chain(i18n::Lang::all().map(i18n::Lang::code)).collect::<Vec<_>>().join(", ");
    assert_eq!(error, format!("language must be one of {codes}"));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "en");
}

#[cfg(target_os = "linux")]
fn start_autoscroll(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> egui::Pos2 {
    let p = h.state().views[0].viewport_rect().center();
    ok(h, c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(h.state().views[0].auto_scrolling());
    p
}

/// Measure document movement from rendered geometry without requiring an offscreen grid
/// cell to remain instantiated. Learn the grid's column count and row pitch before scrolling;
/// any currently rendered cell then identifies the same content origin. This tests actual
/// layout displacement, independently of the autoscroll velocity calculation.
#[cfg(target_os = "linux")]
struct CanvasPosition {
    grid: Option<(usize, f32)>,
}

#[cfg(target_os = "linux")]
impl CanvasPosition {
    fn new(h: &Harness<'static, PdfCraftApp>) -> Self {
        let grid = h.state().views[0].organize.then(|| {
            let first = h.get_by_label("Page 1").rect().top();
            let (page, next_row) = Self::grid_cells(h).into_iter().find(|(_, top)| *top > first + 1.0).expect("a second grid row");
            (page - 1, next_row - first)
        });
        Self { grid }
    }

    fn grid_cells(h: &Harness<'static, PdfCraftApp>) -> Vec<(usize, f32)> {
        use egui_kittest::kittest::NodeT;
        let mut cells: Vec<_> = h
            .query_all_by_label_contains("Page ")
            .filter_map(|node| {
                let page = node.accesskit_node().label()?.strip_prefix("Page ")?.parse::<usize>().ok()?;
                Some((page, node.rect().top()))
            })
            .collect();
        cells.sort_unstable_by_key(|(page, _)| *page);
        cells
    }

    fn top(&self, h: &Harness<'static, PdfCraftApp>) -> f32 {
        match self.grid {
            Some((columns, row_pitch)) => {
                let (page, top) = Self::grid_cells(h).into_iter().next().expect("a rendered grid cell");
                top - ((page - 1) / columns) as f32 * row_pitch
            }
            None => h.state().views[0].page_screen_rect(0).expect("the zoomed viewer page remains visible").top(),
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn middle_click_autoscroll_latches_has_a_dead_zone_and_scrolls_both_directions() {
    let (mut h, c) = harness();
    // Keep the tracked page visible through the downward leg and reversal.
    h.set_option("zoom", "400").unwrap();
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(2);
    let p = start_autoscroll(&mut h, &c);
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    ok(&mut h, &c, "ui.move", json!({ "x": p.x + 100.0, "y": p.y + 8.0 }));
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(0).unwrap().top(), top, "horizontal motion and the dead zone do not scroll");
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 90.0 }));
    h.run_steps(8);
    let down = h.state().views[0].page_screen_rect(0).unwrap().top();
    assert!(down < top - 30.0, "moving below the anchor scrolls down: {top} -> {down}");
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y - 50.0 }));
    // The control request itself takes frames while the previous downward motion continues.
    // Measure reversal after it has arrived and the scroll area's layout has caught up.
    h.run_steps(2);
    let reversing = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    let up = h.state().views[0].page_screen_rect(0).unwrap().top();
    assert!(up > reversing + 15.0, "moving above the anchor scrolls up: {reversing} -> {up}");
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "a second wheel click stops");
    h.run_steps(2);
    let stopped = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(0).unwrap().top(), stopped, "no drift after cancellation");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "scrolling never edits the PDF");
}

#[cfg(target_os = "linux")]
#[test]
fn moving_before_middle_button_release_keeps_scrolling_without_starting_page_tools() {
    let (mut h, c) = harness();
    // Keep the tracked page on screen while the faster gesture continues after release.
    h.set_option("zoom", "400").unwrap();
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(2);
    // A middle drag must not also draw with a selected tool (egui accepts any drag button).
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Crop;
    let p = h.state().views[0].viewport_rect().center();
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    drag(&mut h, &c, json!([p.x, p.y]), json!([p.x, p.y + 60.0]), 12, &[], "middle");
    h.run_steps(2);
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0);
    assert!(!h.state().views[0].auto_scrolling(), "releasing after holding and scrolling turns auto-scroll off");
    let released = h.state().views[0].page_screen_rect(0).unwrap().top();
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(0).unwrap().top(), released, "no drift after releasing a held middle drag");
    // A drag the Crop tool received ends in a crop edit or opens Set Page Boxes.
    assert!(!crate::support::dialog_open(&h), "the Crop tool must not receive a wheel drag");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "the Crop tool must not receive a wheel drag");

    // Small movement within the dead zone before release still latches auto-scroll on.
    drag(&mut h, &c, json!([p.x, p.y]), json!([p.x, p.y + 10.0]), 4, &[], "middle");
    assert!(h.state().views[0].auto_scrolling(), "release within the dead zone keeps scrolling toggled on");
    ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 60.0 }));
    h.run_steps(8);
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < released - 30.0, "scrolling continues with no button held");
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y + 60.0, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "the next middle click toggles scrolling off");
    h.run_steps(2);
    assert!(!crate::support::dialog_open(&h), "the Crop tool must not receive the latched gesture");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "the Crop tool must not receive the latched gesture");
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_uses_the_initial_click_position_when_input_arrives_in_one_frame() {
    let (mut h, c) = harness();
    let p = h.state().views[0].viewport_rect().center();
    let moved = p + egui::vec2(0.0, 60.0);
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    // The harness runs each queued `event` in a separate frame; raw input keeps this batch together.
    h.input_mut().events.extend([
        egui::Event::PointerMoved(p),
        egui::Event::PointerButton { pos: p, button: egui::PointerButton::Middle, pressed: true, modifiers: egui::Modifiers::NONE },
        egui::Event::PointerMoved(moved),
        egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: egui::Modifiers::NONE },
    ]);
    h.run_steps(8);
    assert!(h.state().views[0].auto_scrolling());
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0, "distance is measured from the click, not the last mouse event");
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_uses_elapsed_frame_time_and_preserves_fractional_motion_in_both_views() {
    for (organize, scale, zoom) in [(false, 1.0, "100"), (false, 2.0, "100"), (false, 1.0, "400"), (true, 1.0, "100"), (true, 2.0, "100")] {
        for frames in [30, 60, 120, 144] {
            let (mut h, c) = harness_pages_at(40, 1400.0, scale);
            h.set_option("zoom", zoom).unwrap();
            h.state_mut().views[0].organize = organize;
            h.run_steps(3);
            assert_eq!(h.ctx.pixels_per_point(), scale);
            let position = CanvasPosition::new(&h);
            let top = |h: &Harness<'static, PdfCraftApp>| position.top(h);
            let step = |h: &mut Harness<'static, PdfCraftApp>| {
                // Deliberately differ from the harness's predicted frame interval: scrolling
                // must follow the elapsed time rather than the display's predicted rate.
                h.input_mut().time = Some(h.ctx.input(|i| i.time) + 1.0 / f64::from(frames));
                h.step();
            };
            let p = start_autoscroll(&mut h, &c);
            for (distance, expected) in [(16.0, 17.83), (50.0, 218.67)] {
                ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + distance }));
                step(&mut h);
                step(&mut h);
                let before = top(&h);
                for _ in 0..frames {
                    step(&mut h);
                }
                let travelled = before - top(&h);
                assert!(
                    (travelled - expected).abs() < 1.0,
                    "organize={organize}, scale={scale}, zoom={zoom}, fps={frames}, distance={distance}, travelled={travelled}"
                );
                assert_eq!(
                    h.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
                    std::time::Duration::ZERO,
                    "motion schedules the next display frame"
                );
            }
            ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 15.0 }));
            step(&mut h);
            let paused = top(&h);
            for _ in 0..frames {
                step(&mut h);
            }
            assert_eq!(top(&h), paused, "the dead zone pauses immediately, without coasting");
            assert!(h.state().views[0].auto_scrolling());
            ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y - 50.0 }));
            step(&mut h);
            step(&mut h);
            let before = top(&h);
            for _ in 0..frames {
                step(&mut h);
            }
            assert!((top(&h) - before - 218.67).abs() < 1.0, "resuming above the anchor reverses direction");
            ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn escape_stops_autoscroll_without_closing_find() {
    let (mut h, c) = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(3);
    // Focus the canvas so the text field no longer owns keyboard input.
    let p = h.state().views[0].viewport_rect().center();
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y }));
    h.run_steps(2);
    start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    assert!(!h.state().views[0].auto_scrolling());
    assert!(h.state().views[0].find.is_some(), "Escape cancels the scrolling gesture first");
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_cancels_on_click_wheel_focus_loss_and_pointer_exit() {
    let (mut h, c) = harness();
    let p = start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y }));
    assert!(!h.state().views[0].auto_scrolling());
    start_autoscroll(&mut h, &c);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -20.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    start_autoscroll(&mut h, &c);
    h.input_mut().focused = false;
    h.event(egui::Event::WindowFocused(false));
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    h.input_mut().focused = true;
    h.event(egui::Event::WindowFocused(true));
    h.run_steps(2);
    start_autoscroll(&mut h, &c);
    h.event(egui::Event::PointerGone);
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
}

#[cfg(target_os = "linux")]
#[test]
fn autoscroll_is_scoped_to_the_active_view_and_cannot_start_under_a_dialog() {
    let (mut h, c) = harness();
    let p = start_autoscroll(&mut h, &c);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, fixture(2)).unwrap();
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling());
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert!(!h.state().views[0].auto_scrolling(), "returning to the tab must not resume");
    start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.set", json!({ "key": "dialog", "value": "about" }));
    settle(&mut h);
    assert!(!h.state().views[0].auto_scrolling());
    ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
    assert!(!h.state().views[0].auto_scrolling(), "a modal owns its input");
    assert!(call(&mut h, &c, "ui.move", json!({ "x": "bad", "y": 2 })).is_err());
    assert!(call(&mut h, &c, "ui.click", json!({ "x": 1, "y": 2, "button": "bad" })).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn the_click_that_stops_autoscroll_preserves_the_page_selection() {
    use egui_kittest::kittest::Queryable;
    let (mut h, c) = harness_pages(40);
    h.execute("page.organize");
    h.state_mut().views[0].select_pages(&[0]);
    h.run_steps(3);
    let page_two = h.get_by_label("Page 2").rect().center();
    start_autoscroll(&mut h, &c);
    ok(&mut h, &c, "ui.click", json!({ "x": page_two.x, "y": page_two.y }));
    assert!(!h.state().views[0].auto_scrolling());
    assert_eq!(h.state().views[0].target_pages(), vec![0], "the cancelling press and release belong to autoscroll");
    // The next ordinary click still selects normally.
    ok(&mut h, &c, "ui.click", json!({ "x": page_two.x, "y": page_two.y }));
    assert_eq!(h.state().views[0].target_pages(), vec![1]);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn middle_button_pans_while_held_outside_linux() {
    for organize in [false, true] {
        let (mut h, c) = harness_pages(40);
        h.state_mut().views[0].organize = organize;
        h.run_steps(3);
        let p = h.state().views[0].viewport_rect().center();
        ok(&mut h, &c, "ui.click", json!({ "x": p.x, "y": p.y, "button": "middle" }));
        ok(&mut h, &c, "ui.move", json!({ "x": p.x, "y": p.y + 50.0 }));
        h.run_steps(8);
        assert!(!h.state().views[0].auto_scrolling(), "a click does not latch auto-scroll outside Linux: organize={organize}");
        assert!(!h.state().views[0].middle_panning(), "a released click leaves nothing to pan: organize={organize}");
    }
    // A drag pans while the button is held, and the Crop tool never sees it.
    let (mut h, c) = harness();
    h.set_option("zoom", "400").unwrap();
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Crop;
    let p = h.state().views[0].viewport_rect().center();
    let top = h.state().views[0].page_screen_rect(1).unwrap().top();
    drag(&mut h, &c, json!([p.x, p.y]), json!([p.x, p.y - 60.0]), 12, &[], "middle");
    h.run_steps(2);
    let moved = h.state().views[0].page_screen_rect(1).unwrap().top();
    assert!((moved - (top - 60.0)).abs() < 1.0, "the page follows the pointer 1:1: {top} -> {moved}");
    h.run_steps(8);
    assert_eq!(h.state().views[0].page_screen_rect(1).unwrap().top(), moved, "no drift after release");
    assert!(h.state().views[0].crop_drag.is_none(), "the Crop tool must not receive a wheel drag");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

/// With Invert middle-button drag on, a held middle drag moves the view the way the pointer goes.
#[cfg(not(target_os = "linux"))]
#[test]
fn inverted_middle_drag_moves_the_view_the_way_the_pointer_goes() {
    let (mut h, c) = harness();
    h.set_option("zoom", "400").unwrap();
    ok(&mut h, &c, "ui.set", json!({ "key": "invert_middle_drag", "value": "true" }));
    h.run_steps(3);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    let p = h.state().views[0].viewport_rect().center();
    let top = h.state().views[0].page_screen_rect(1).unwrap().top();
    drag(&mut h, &c, json!([p.x, p.y]), json!([p.x, p.y - 60.0]), 12, &[], "middle");
    h.run_steps(2);
    let moved = h.state().views[0].page_screen_rect(1).unwrap().top();
    assert!((moved - (top + 60.0)).abs() < 1.0, "dragging up scrolls up, so the page moves down: {top} -> {moved}");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

/// In a Japanese interface, the mode tabs and Preferences show Japanese labels, the command
/// palette finds commands by their Japanese or English names or their ids, and switching back to
/// English keeps the tabs working.
#[test]
fn japanese_controls_and_search_keep_command_ids() {
    if crate::support::skip_without_catalogs("japanese_controls_and_search_keep_command_ids") {
        return;
    }
    if crate::support::skip_without_glyphs("japanese_controls_and_search_keep_command_ids", "ja", "") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "ja" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "ja");
    ok(&mut h, &c, "ui.click", json!({ "label": "変換" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Convert");

    ok(&mut h, &c, "ui.command", json!({ "id": "app.preferences" }));
    // The command sets the dialog; its content reaches the accessibility tree in the next pass.
    h.run_steps(2);
    let prefs = ok(&mut h, &c, "ui.inspect", json!({ "query": "表示言語" }));
    assert!(prefs["count"].as_u64().unwrap() > 0, "{prefs}");
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));

    for (query, translated) in [("整理", "ページを整理"), ("Split document", "文書を分割…"), ("page.split", "文書を分割…")] {
        ok(&mut h, &c, "ui.command", json!({ "id": "view.palette" }));
        ok(&mut h, &c, "ui.type", json!({ "text": query }));
        let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": translated }));
        assert!(hits["count"].as_u64().unwrap() > 0, "{query}: {hits}");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        // Escape cleared the query; on the empty query it closes the palette.
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    }
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "en" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "All tools" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "AllTools");
}

/// In a German interface, the mode tabs and Preferences show German labels, the language
/// selector switches to English and back, and the command palette finds Split document by its
/// German name, its English name or its command id.
#[test]
fn german_preferences_and_search_keep_command_ids() {
    if crate::support::skip_without_catalogs("german_preferences_and_search_keep_command_ids") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "de" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "Konvertieren" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Convert");
    ok(&mut h, &c, "ui.command", json!({ "id": "app.preferences" }));
    h.run_steps(2);
    let prefs = ok(&mut h, &c, "ui.inspect", json!({ "query": "Sprache der Oberfläche" }));
    assert!(prefs["count"].as_u64().unwrap() > 0, "{prefs}");
    let selector = ok(&mut h, &c, "ui.inspect", json!({ "query": "Deutsch" }));
    let combo = selector["widgets"].as_array().unwrap().iter().find(|w| w["role"] == "ComboBox").expect("language selector");
    ok(&mut h, &c, "ui.click", json!({ "id": combo["id"] }));
    ok(&mut h, &c, "ui.click", json!({ "label": "English" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "en");
    let selector = ok(&mut h, &c, "ui.inspect", json!({ "query": "English" }));
    let combo = selector["widgets"].as_array().unwrap().iter().find(|w| w["role"] == "ComboBox").expect("language selector");
    ok(&mut h, &c, "ui.click", json!({ "id": combo["id"] }));
    ok(&mut h, &c, "ui.click", json!({ "label": "Deutsch" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], "de");
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));
    for query in ["Dokument teilen", "Split document", "page.split"] {
        ok(&mut h, &c, "ui.command", json!({ "id": "view.palette" }));
        ok(&mut h, &c, "ui.type", json!({ "text": query }));
        let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": "Dokument teilen…" }));
        assert!(hits["count"].as_u64().unwrap() > 0, "{query}: {hits}");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        // Escape cleared the query; on the empty query it closes the palette.
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    }
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");
}

/// In a Ukrainian interface, Preferences and dialogs show Ukrainian labels, the command palette
/// finds Split document by its Ukrainian name, its English name or its command id, and a refused
/// edit's notice names the command in Ukrainian.
#[test]
fn ukrainian_preferences_and_search_keep_command_ids() {
    if crate::support::skip_without_catalogs("ukrainian_preferences_and_search_keep_command_ids") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "uk" }));
    ok(&mut h, &c, "ui.command", json!({ "id": "app.preferences" }));
    h.run_steps(2);
    for label in ["Мова інтерфейсу", "Українська"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({ "query": label }));
        assert!(found["count"].as_u64().unwrap() > 0, "{label}: {found}");
    }
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));
    for query in ["Розділити", "Split document", "page.split"] {
        ok(&mut h, &c, "ui.command", json!({ "id": "view.palette" }));
        ok(&mut h, &c, "ui.type", json!({ "text": query }));
        let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": "Розділити документ…" }));
        assert!(hits["count"].as_u64().unwrap() > 0, "{query}: {hits}");
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
        // Escape cleared the query; on the empty query it closes the palette.
        ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    }
    for (dialog, label) in [("properties", "Властивості документа"), ("protect", "Захистити паролем"), ("about", "Учасники")]
    {
        ok(&mut h, &c, "ui.set", json!({ "key": "dialog", "value": dialog }));
        let found = ok(&mut h, &c, "ui.inspect", json!({ "query": label }));
        assert!(found["count"].as_u64().unwrap() > 0, "{dialog}: {found}");
    }
    ok(&mut h, &c, "ui.set", json!({ "key": "dialog", "value": "none" }));
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0, 1, 2, 3, 4] }));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["language"], "uk");
    assert_eq!(state["notice"], "Помилка «Видалити сторінки»: a document must keep at least one page");
    assert_eq!(state["documents"][0]["name"], "doc.pdf");
}

/// In a Japanese interface, Preferences opened from the menu bar switches the language to English,
/// Ukrainian and back, and opens again with Cmd+,; the shortcuts list then reads in Japanese and
/// the document stays open. Preferences is in Edit on Windows and Linux, and in the app menu (as
/// Settings…) on macOS.
#[test]
fn preferences_menu_and_shortcut_allow_switching_interface_languages() {
    if crate::support::skip_without_catalogs("preferences_menu_and_shortcut_allow_switching_interface_languages") {
        return;
    }
    if crate::support::skip_without_glyphs("preferences_menu_and_shortcut_allow_switching_interface_languages", "ja", "") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "language", "value": "ja" }));
    // The menu bar's Edit menu holds Preferences on Windows and Linux. On macOS the in-window bar
    // takes the Mac layout (`Target::current()`), which moves it to the app menu as Settings….
    // Titles and rows carry their mnemonic ("編集(E)", "環境設定(P)…"), so match by the start of
    // the label.
    let (menu, preferences) = if cfg!(target_os = "macos") {
        let ja = pdfcraft_ui_egui::i18n::Lang::from_code("ja").unwrap();
        ("PdfCraft", pdfcraft_ui_egui::i18n::tr(ja, "Settings…").trim_end_matches('…'))
    } else {
        ("編集", "環境設定")
    };
    let menu_row = |text: &'static str| {
        move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == egui::accesskit::Role::MenuItem && n.label().is_some_and(|l| l.starts_with(text))
        }
    };
    h.get_by(menu_row(menu)).click();
    h.run_steps(2);
    h.get_by(menu_row(preferences)).click();
    h.run_steps(2);
    for (current, next, code) in [("日本語", "English", "en"), ("English", "Українська", "uk"), ("Українська", "日本語", "ja")]
    {
        let selector = ok(&mut h, &c, "ui.inspect", json!({ "query": current }));
        let combo = selector["widgets"].as_array().unwrap().iter().find(|w| w["role"] == "ComboBox").expect("language selector");
        ok(&mut h, &c, "ui.click", json!({ "id": combo["id"] }));
        ok(&mut h, &c, "ui.click", json!({ "label": next }));
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["language"], code);
    }
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));
    ok(&mut h, &c, "ui.key", json!({ "key": ",", "modifiers": {"command": true} }));
    let prefs = ok(&mut h, &c, "ui.inspect", json!({ "query": "表示言語" }));
    assert!(prefs["count"].as_u64().unwrap() > 0, "{prefs}");
    ok(&mut h, &c, "ui.click", json!({ "label": "OK" }));

    ok(&mut h, &c, "ui.command", json!({ "id": "help.shortcuts" }));
    for label in ["キーボードショートカット", "開く", "環境設定", "次／前の検索結果", "ダブルクリック", "閉じる"]
    {
        let found = ok(&mut h, &c, "ui.inspect", json!({ "query": label }));
        assert!(found["count"].as_u64().unwrap() > 0, "{label}: {found}");
    }
    let english = ok(&mut h, &c, "ui.inspect", json!({ "query": "Next / previous match" }));
    assert_eq!(english["count"], 0);
    ok(&mut h, &c, "ui.click", json!({ "label": "閉じる" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");
}

#[test]
fn japanese_dialogs_errors_and_custom_action_names() {
    if crate::support::skip_without_catalogs("japanese_dialogs_errors_and_custom_action_names") {
        return;
    }
    if crate::support::skip_without_glyphs("japanese_dialogs_errors_and_custom_action_names", "ja", "") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key": "language", "value": "ja"}));
    for (dialog, label) in [
        ("properties", "文書のプロパティ"),
        ("protect", "パスワードで保護"),
        ("export-image", "画像に書き出し"),
        ("optimize", "PDF の最適化"),
        ("recognize-text", "テキストを認識"),
        ("accessibility-options", "アクセシビリティチェックのオプション"),
        ("js-console", "JavaScript コンソール"),
        ("compare-files", "ファイルを比較"),
        ("sign", "署名用のデジタル ID を設定"),
    ] {
        ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": dialog}));
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["count"].as_u64().unwrap() > 0, "{dialog}: {found}");
    }
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "none"}));
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0, 1, 2, 3, 4] }));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    // The frame is translated; the engine's own error text is shown as it is.
    assert_eq!(state["notice"], "操作「ページを削除」に失敗しました: a document must keep at least one page");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"][0]["name"], "doc.pdf");

    h.state_mut().custom_actions.push(pdfcraft_engine::actions::Action {
        name: "Save".to_string(),
        description: "Print".to_string(),
        steps: Vec::new(),
        builtin: false,
    });
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "action-wizard"}));
    settle(&mut h);
    ok(&mut h, &c, "ui.click", json!({"label": "Save"}));
    for label in ["アクションウィザード", "Save", "Print"] {
        let found = ok(&mut h, &c, "ui.inspect", json!({"query": label}));
        assert!(found["widgets"].as_array().unwrap().iter().any(|w| w["label"] == label || w["value"] == label), "{found}");
    }
}

#[test]
fn simplified_chinese_signature_prompts_and_errors_keep_document_state() {
    if crate::support::skip_without_catalogs("simplified_chinese_signature_prompts_and_errors_keep_document_state") {
        return;
    }
    if crate::support::skip_without_glyphs("simplified_chinese_signature_prompts_and_errors_keep_document_state", "zh-hans", "") {
        return;
    }
    let (mut h, c) = harness();
    let documents = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    ok(&mut h, &c, "ui.set", json!({"key": "language", "value": "zh-hans"}));
    settle(&mut h);
    ok(&mut h, &c, "ui.set", json!({"key": "dialog", "value": "signature"}));
    settle(&mut h);
    let typed = ok(&mut h, &c, "ui.inspect", json!({"query": "请输入您的签名。"}));
    assert!(typed["count"].as_u64().unwrap() > 0, "{typed}");
    // The shell also has a Draw control; the modal's button is registered after the shell.
    let draw = ok(&mut h, &c, "ui.inspect", json!({"query": "绘制"}));
    let id = draw["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|w| w["label"] == "绘制" && w["clickable"] == true)
        .expect("signature Draw button")["id"]
        .clone();
    ok(&mut h, &c, "ui.click", json!({"id": id}));
    let drawn = ok(&mut h, &c, "ui.inspect", json!({"query": "请在下方绘制您的签名。"}));
    assert!(drawn["count"].as_u64().unwrap() > 0, "{drawn}");
    ok(&mut h, &c, "ui.click", json!({"label": "取消"}));
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0, 1, 2, 3, 4] }));
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["notice"], "删除页面失败：a document must keep at least one page");
    assert_eq!(state["documents"], documents, "a language change and failed edit must preserve the document");
}

#[test]
fn cover_page_command_needs_two_page_view() {
    // Agents see the cover toggle as disabled, and get an error, until two-page view.
    let (mut h, c) = harness();
    let enabled = |list: Value| list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "view.layout.cover").unwrap()["enabled"].clone();
    assert_eq!(enabled(ok(&mut h, &c, "ui.commands", json!({}))), false);
    let err = call(&mut h, &c, "ui.command", json!({ "id": "view.layout.cover" })).unwrap_err();
    assert!(err.contains("disabled"), "{err}");
    ok(&mut h, &c, "ui.command", json!({ "id": "view.layout.two_up" }));
    assert_eq!(enabled(ok(&mut h, &c, "ui.commands", json!({}))), true);
    ok(&mut h, &c, "ui.command", json!({ "id": "view.layout.cover" }));
    assert!(h.state().views[0].view.cover);
}

#[test]
fn commands_keys_and_typing() {
    let (mut h, c) = harness();
    let list = ok(&mut h, &c, "ui.commands", json!({}));
    let undo = list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "edit.undo").unwrap().clone();
    assert_eq!(undo["enabled"], false);
    assert!(call(&mut h, &c, "ui.command", json!({ "id": "edit.undo" })).unwrap_err().contains("disabled"));
    assert!(call(&mut h, &c, "ui.command", json!({ "id": "nope" })).is_err());

    ok(&mut h, &c, "ui.command", json!({ "id": "page.rotate" }));
    h.run_steps(2);
    let list = ok(&mut h, &c, "ui.commands", json!({}));
    assert_eq!(list["commands"].as_array().unwrap().iter().find(|x| x["id"] == "edit.undo").unwrap()["enabled"], true);

    // ⌘K opens the palette; typing filters it; Escape closes it.
    ok(&mut h, &c, "ui.key", json!({ "key": "K", "modifiers": {"command": true} }));
    assert!(palette_on_screen(&h), "the palette is on screen");
    ok(&mut h, &c, "ui.type", json!({ "text": "split" }));
    let hits = ok(&mut h, &c, "ui.inspect", json!({ "query": "split document" }));
    assert!(hits["count"].as_u64().unwrap() >= 1, "{hits}");
    // First Escape clears the query; second closes the palette.
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    ok(&mut h, &c, "ui.key", json!({ "key": "Escape" }));
    h.run_steps(2);
    assert!(!palette_on_screen(&h), "Escape closes the palette");

    assert!(call(&mut h, &c, "ui.key", json!({ "key": "NotAKey" })).is_err());
    assert!(call(&mut h, &c, "ui.frobnicate", json!({})).unwrap_err().contains("unknown method"));
}

#[test]
fn ctrl_dragging_a_thumbnail_through_the_control_channel_copies_the_page() {
    let (mut h, c) = harness_pages(3);
    ok(&mut h, &c, "ui.set", json!({ "key": "tool", "value": "organize" }));
    h.run_steps(3);
    // The grid cell of a page: the node labelled exactly "Page N" (the checkbox says "Select page N").
    let cell = |h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, page: usize| -> [f64; 4] {
        let nodes = ok(h, c, "ui.inspect", json!({ "query": &format!("page {page}") }));
        let want = format!("Page {page}");
        let r = nodes["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["label"].as_str() == Some(want.as_str()))
            .and_then(|w| w["rect"].as_array().cloned())
            .expect("the page's grid cell");
        let f = |i: usize| r[i].as_f64().unwrap();
        [f(0), f(1), f(2), f(3)]
    };
    let (a, b) = (cell(&mut h, &c, 1), cell(&mut h, &c, 3));
    // The gesture reads the OS the context reports: Option on macOS, Ctrl elsewhere.
    let copy = if h.ctx.os() == egui::os::OperatingSystem::Mac { "alt" } else { "ctrl" };
    drag(&mut h, &c, json!([(a[0] + a[2]) / 2.0, (a[1] + a[3]) / 2.0]), json!([b[2] - 4.0, (b[1] + b[3]) / 2.0]), 8, &[copy], "left");
    h.run_steps(3);
    let st = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(st["documents"][0]["pages"], 4, "the drag copied the page instead of moving it: {st}");
    assert_eq!(st["active"]["selected_pages"], json!([4]), "{st}");
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3", "Page 1"], "the copy landed at the drop gap, after page 3");
}

#[test]
fn organize_overview_is_reachable_through_the_control_channel() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({ "key": "tool", "value": "organize" }));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["active"]["organize"], true);
    h.run_steps(3);
    for label in ["Select 1", "Select 3"] {
        ok(&mut h, &c, "ui.click", json!({ "label": label }));
        h.run_steps(3);
    }
    let page = h.get_by_label("Page 1").rect().center();
    ok(&mut h, &c, "ui.move", json!({ "x": page.x, "y": page.y }));
    ok(&mut h, &c, "ui.click", json!({ "label": "Rotate page 1 clockwise" }));
    h.run_steps(3);
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["active"]["selected_pages"], json!([1, 3]));
    let app = h.state();
    let rotations: Vec<_> = app.session.get(app.views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect();
    assert_eq!(rotations, [90, 0, 0, 0, 0]);
}

#[test]
fn saved_signature_can_be_changed_through_the_control_channel() {
    let (mut h, c) = harness();
    h.state_mut().signature = Some(craft::review::SavedSignature::Typed("Ada Lovelace".into()));
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.signature.change" }));
    h.run_steps(3);
    // The pad opens on the saved text: its text field shows it.
    let fields = ok(&mut h, &c, "ui.inspect", json!({ "role": "TextInput" }));
    let field = fields["widgets"].as_array().unwrap().iter().find(|w| w["value"] == "Ada Lovelace").expect("the signature text field");
    let r = field["rect"].as_array().unwrap();
    let [x0, y0, x1, y1] = [0, 1, 2, 3].map(|i| r[i].as_f64().unwrap());
    ok(&mut h, &c, "ui.click", json!({ "x": (x0 + x1) / 2.0, "y": (y0 + y1) / 2.0 }));
    ok(&mut h, &c, "ui.key", json!({ "key": "A", "modifiers": {"command": true} }));
    ok(&mut h, &c, "ui.type", json!({ "text": "Grace Hopper" }));
    ok(&mut h, &c, "ui.click", json!({ "label": "Apply" }));
    assert_eq!(h.state().signature, Some(craft::review::SavedSignature::Typed("Grace Hopper".into())));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["quick_tool"], "fill-signature");
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.signature.remove" }));
    assert_eq!(h.state().signature, None);
    h.state_mut().initials = Some(craft::review::SavedSignature::Typed("GH".into()));
    ok(&mut h, &c, "ui.command", json!({ "id": "sign.fill.initials.remove" }));
    assert_eq!(h.state().initials, None);
}

#[test]
fn drawing_a_comment_by_drag_and_its_context_menu() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.command", json!({ "id": "comment.square" }));
    let st = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(st["quick_tool"], "square");
    let r = &st["active"]["pages_on_screen"][0]["rect"];
    let (x0, y0, x1, y1) = (r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap());
    let (a, b) = ([x0 + (x1 - x0) * 0.2, y0 + (y1 - y0) * 0.2], [x0 + (x1 - x0) * 0.5, y0 + (y1 - y0) * 0.4]);
    drag(&mut h, &c, json!(a), json!(b), 8, &[], "left");
    h.run_steps(2);
    let st = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(st["active"]["selected_comment"], json!({ "page": 1, "index": 1 }), "{st}");
    assert_eq!(st["documents"][0]["dirty"], true);
    // A right-click on it offers the comment menu.
    ok(&mut h, &c, "ui.click", json!({ "x": (a[0] + b[0]) / 2.0, "y": (a[1] + b[1]) / 2.0, "button": "secondary" }));
    let menu = ok(&mut h, &c, "ui.inspect", json!({ "query": "Set status" }));
    assert!(menu["count"].as_u64().unwrap() >= 1, "{menu}");
    assert!(call(&mut h, &c, "ui.move", json!({ "x": 1 })).is_err());
}

#[test]
fn control_default_workspace_and_session_override() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key": "default-mode", "value": "edit"}));
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["default_mode"], "edit");
    assert!(call(&mut h, &c, "ui.set", json!({"key": "default-mode", "value": "unknown"})).is_err());
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "next.pdf", None, fixture(1)).unwrap();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Edit");
    ok(&mut h, &c, "ui.set", json!({"key": "mode", "value": "read"}));
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "another.pdf", None, fixture(1)).unwrap();
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["mode"], "Read");
}

#[test]
fn measurement_tools_draw_live_calibrate_save_and_export() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-measure-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("measurements.csv");
    // The CSV's Save pick is the only picker the test meets; the script answers it.
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![csv.clone()])]));
    let (mut h, c) = harness_with_files(1, files.clone());
    let doc = h.state().views[0].id;
    h.set_option("zoom", "100").unwrap();
    ok(&mut h, &c, "ui.command", json!({"id":"measure.scale"}));
    h.run_steps(3);
    h.state_mut().views[0].session.measure.drawing_points = 10.0;
    h.state_mut().views[0].session.measure.page_unit = Some(pdfcraft_engine::measure::PageUnit::Pt);
    h.state_mut().views[0].session.measure.real_distance = 1.0;
    h.state_mut().views[0].session.measure.unit = "m".into();
    h.get_by_label("Apply scale").click_accesskit();
    h.run_steps(3);
    let scale = h.state().session.get(doc).unwrap().measurement_scale(0, [20.0, 20.0]).unwrap();
    assert!((scale.x - 0.1).abs() < 1e-10);
    let click = |h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, x: f32, y: f32| {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        ok(h, c, "ui.click", json!({"x":r.left()+x*r.width()/200.0,"y":r.top()+y*r.height()/300.0}));
        h.run_steps(2);
    };
    for (command, points) in [
        ("measure.distance", vec![(20.0, 30.0), (80.0, 110.0)]),
        ("measure.perimeter", vec![(20.0, 130.0), (80.0, 130.0), (80.0, 210.0)]),
        ("measure.area", vec![(100.0, 130.0), (160.0, 130.0), (160.0, 210.0), (100.0, 210.0)]),
    ] {
        ok(&mut h, &c, "ui.command", json!({"id":command}));
        for (x, y) in points {
            click(&mut h, &c, x, y);
        }
        if command != "measure.distance" {
            ok(&mut h, &c, "ui.key", json!({"key":"Enter"}));
            h.run_steps(3);
        }
    }
    let measurements = h.state().session.get(doc).unwrap().measurements().unwrap().measurements;
    assert_eq!(measurements.len(), 3);
    for (m, value) in measurements.iter().zip([10.0, 14.0, 48.0]) {
        assert!((m.reading.value - value).abs() < 0.01, "{m:?}");
    }
    ok(&mut h, &c, "ui.command", json!({"id":"edit.undo"}));
    h.run_steps(2);
    assert_eq!(h.state().session.get(doc).unwrap().measurements().unwrap().measurements.len(), 2);
    ok(&mut h, &c, "ui.command", json!({"id":"edit.redo"}));
    h.run_steps(2);
    ok(&mut h, &c, "ui.command", json!({"id":"measure.export"}));
    h.run_steps(2);
    assert_eq!(files.requests().len(), 1, "the export asked where to save once");
    // The file service keeps what it is given in memory; a writer that went to the disk instead
    // left the file in the folder.
    let written = files.file(&csv).or_else(|| std::fs::read(&csv).ok()).expect("the CSV was written where the pick said");
    assert!(String::from_utf8(written).unwrap().contains("m^2"));
    h.set_option("quick", "measure-calibrate").unwrap();
    click(&mut h, &c, 30.0, 50.0);
    click(&mut h, &c, 130.0, 50.0);
    assert!((h.state().views[0].session.measure.drawing_points - 100.0).abs() < 0.01);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_1_to_50_scale_in_millimetres_measures_a_10_mm_line_as_500_mm() {
    // #2752: a plan drawn at 1:50 is "1 mm = 50 mm". The page side of the scale was fixed in
    // points, so this stored 1 pt = 50 mm, 72 / 25.4 = 2.835 times too big.
    use pdfcraft_engine::measure::{self, Kind};
    let (mut h, c) = harness_pages(1);
    let doc = h.state().views[0].id;
    h.set_option("zoom", "100").unwrap();
    ok(&mut h, &c, "ui.command", json!({"id":"measure.scale"}));
    h.run_steps(3);
    let m = &mut h.state_mut().views[0].session.measure;
    (m.drawing_points, m.real_distance, m.unit) = (1.0, 50.0, "mm".into());
    h.get_by_label("Apply scale").click_accesskit();
    h.run_steps(3);
    let mm = 72.0 / 25.4;
    let line = [[20.0, 30.0], [20.0 + 10.0 * mm, 30.0]];
    let stored = |h: &Harness<'static, PdfCraftApp>| {
        let scale = h.state().session.get(doc).unwrap().measurement_scale(0, line[0]).unwrap();
        (measure::reading(Kind::Distance, &line, &scale).unwrap().value, scale.ratio)
    };
    let (value, ratio) = stored(&h);
    assert!((value - 500.0).abs() < 1e-9, "a 10 mm line at 1:50 reads {value} ({ratio})");
    assert_eq!(ratio, "1 mm = 50 mm");
    // The scale is one undoable edit.
    ok(&mut h, &c, "ui.command", json!({"id":"edit.undo"}));
    h.run_steps(2);
    assert!((stored(&h).0 - 500.0).abs() > 1.0);
    ok(&mut h, &c, "ui.command", json!({"id":"edit.redo"}));
    h.run_steps(2);
    assert_eq!(stored(&h), (value, ratio));
    // Measuring the line on the page with the Distance tool reads the same.
    ok(&mut h, &c, "ui.command", json!({"id":"measure.distance"}));
    for [x, y] in line {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        ok(&mut h, &c, "ui.click", json!({"x":r.left()+x as f32*r.width()/200.0,"y":r.top()+y as f32*r.height()/300.0}));
        h.run_steps(2);
    }
    let measurements = h.state().session.get(doc).unwrap().measurements().unwrap().measurements;
    assert_eq!(measurements.len(), 1);
    assert!((measurements[0].reading.value - 500.0).abs() < 0.05, "{:?}", measurements[0].reading);
    assert_eq!(measurements[0].reading.unit, "mm");
}

#[test]
fn a_plan_scale_picked_or_typed_applies_at_once() {
    use pdfcraft_engine::measure::{self, Kind};
    let (mut h, c) = harness_pages(1);
    let doc = h.state().views[0].id;
    ok(&mut h, &c, "ui.command", json!({"id":"measure.scale"}));
    h.run_steps(3);
    h.state_mut().views[0].session.measure.unit = "m".into();
    h.run_steps(2);
    // 60 mm of paper.
    let line = [[20.0, 20.0], [20.0 + 60.0 / 25.4 * 72.0, 20.0]];
    let read = |h: &Harness<'static, PdfCraftApp>| {
        let scale = h.state().session.get(doc).unwrap().measurement_scale(0, line[0]).unwrap();
        (measure::reading(Kind::Distance, &line, &scale).unwrap().label, scale.ratio)
    };
    // Picked from the common scales: no Apply click.
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Common scales").click();
    h.run_steps(2);
    h.get_by_label("1:200").click_accesskit();
    h.run_steps(3);
    assert_eq!(read(&h), ("12.00 m".to_string(), "1 mm = 0.2 m".to_string()));
    // Typed, in the other notations; leaving the field applies.
    fn field<'a>(h: &'a Harness<'static, PdfCraftApp>) -> egui_kittest::Node<'a> {
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Drawing scale")
    }
    for (typed, label) in [("1/50", "3.00 m"), ("1:100", "6.00 m")] {
        field(&h).click();
        h.run_steps(1);
        h.state_mut().views[0].session.measure.ratio_text.clear();
        field(&h).type_text(typed);
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(read(&h).0, label, "{typed}");
    }
    // Nonsense is refused with a message and leaves the page scale alone.
    let before = read(&h);
    field(&h).click();
    h.run_steps(1);
    h.state_mut().views[0].session.measure.ratio_text.clear();
    field(&h).type_text("1:0");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(read(&h), before);
    assert!(h.state().views[0].session.measure.error.is_some());
    // The scale is one undoable edit.
    ok(&mut h, &c, "ui.command", json!({"id":"edit.undo"}));
    h.run_steps(2);
    assert_ne!(read(&h), before);
}

/// The dock's state, as `ui.panel` reports it when it is asked to change nothing.
fn dock(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> Value {
    ok(h, c, "ui.panel", json!({"panel": "side"}))["side_panels"].clone()
}

#[test]
fn ui_panel_opens_closes_and_sizes_panes_and_survives_restore() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key":"panel", "value":"bookmarks"}));
    h.run_steps(3);
    // Sizes are the dock's: dragged dividers or `ui.dock.apply`, never a `ui.panel` parameter.
    assert!(call(&mut h, &c, "ui.panel", json!({"panel":"side", "width": 401.6})).is_err());
    ok(&mut h, &c, "ui.panel", json!({"panel":"tools", "open": false}));
    h.run_steps(3);
    assert_eq!(dock(&mut h, &c)["tools_open"], false);
    ok(&mut h, &c, "ui.panel", json!({"panel":"side", "open": false}));
    h.run_steps(3);
    assert_eq!(dock(&mut h, &c)["side_open"], false);
    assert!(h.query_by_label("This document has no bookmarks.").is_none());
    // Reopening the side panel brings back the one open last.
    ok(&mut h, &c, "ui.panel", json!({"panel":"side", "open": true}));
    h.run_steps(3);
    h.get_by_label("This document has no bookmarks.");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["side_panels"]["side_panel"], "Bookmarks");
    // What the app would save holds the layout in its dock section; a fresh app that starts from it
    // shows the same panes.
    let saved = h.state().settings_snapshot();
    assert!(saved.dock.dock.is_some(), "the layout is in the saved dock section");
    let (mut reloaded, client) = harness();
    reloaded.state_mut().apply_settings(&saved);
    reloaded.run_steps(4);
    assert_eq!(ok(&mut reloaded, &client, "ui.state", json!({}))["side_panels"], dock(&mut h, &c), "the restored panes are the saved ones");
    // The ui.command form and the reset command.
    ok(&mut reloaded, &client, "ui.command", json!({"id":"ui.panel", "params":{"panel":"tools", "open":true}}));
    assert_eq!(dock(&mut reloaded, &client)["tools_open"], true);
    ok(&mut reloaded, &client, "ui.set", json!({"key":"tools-side", "value":"right"}));
    assert_eq!(dock(&mut reloaded, &client)["tools_side"], "right");
    ok(&mut reloaded, &client, "ui.command", json!({"id":"ui.dock.reset"}));
    let state = ok(&mut reloaded, &client, "ui.state", json!({}))["side_panels"].clone();
    assert_eq!((state["tools_side"].clone(), state["tools_open"].clone()), (json!("left"), json!(true)), "reset puts the Tools pane back");
    assert_eq!(ok(&mut reloaded, &client, "ui.state", json!({}))["documents"][0]["pages"], 5);
}

#[test]
fn invalid_panel_requests_change_nothing() {
    let (mut h, c) = harness();
    let before = dock(&mut h, &c);
    for params in [
        json!({"panel":"canvas"}),
        json!({"panel":"inspector", "open": true}),
        json!({}),
        json!({"panel":"tools", "open":"yes"}),
        json!({"panel":"tools", "width":"wide"}),
        json!({"panel":"side", "width": 300, "open": 1}),
    ] {
        assert!(call(&mut h, &c, "ui.panel", params).is_err());
        assert_eq!(dock(&mut h, &c), before);
        let state = ok(&mut h, &c, "ui.state", json!({}));
        assert_eq!(state["documents"][0]["pages"], 5);
        assert_eq!(state["documents"][0]["dirty"], false);
    }
}

/// #3580 review: a panel a script opens (`app.execMenuItem("ShowHideBookmarks")`) is shown.
#[test]
fn a_script_opened_panel_shows() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key":"panel", "value":"pages"}));
    h.run_steps(3);
    assert!(h.query_by_label("This document has no bookmarks.").is_none());
    let id = h.state().active_ids().unwrap().1;
    h.run_button_script(id, "", "app.execMenuItem('ShowHideBookmarks')");
    assert!(crate::support::step_until(&mut h, |h| h.query_by_label("This document has no bookmarks.").is_some()));
}

#[test]
fn home_and_combine_hide_the_side_panel_without_forgetting_it() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key":"panel", "value":"bookmarks"}));
    let before = dock(&mut h, &c);
    for combine in [false, true] {
        if combine {
            h.state_mut().open_combine_tab();
        } else {
            h.state_mut().active = None;
        }
        h.run_steps(4);
        assert!(h.query_by_label("This document has no bookmarks.").is_none());
        assert_eq!(dock(&mut h, &c)["side_panel"], "Bookmarks", "the panel is remembered while hidden");
        assert_eq!(dock(&mut h, &c), before);
        h.state_mut().active = Some(0);
        h.state_mut().combine_tab.focused = false;
        h.run_steps(4);
        h.get_by_label("This document has no bookmarks.");
        assert_eq!(dock(&mut h, &c), before);
    }
}

/// A saved dock section that does not read leaves the dock at its default and costs the other
/// preferences nothing, whether it came from the last release's file or from this one's.
#[test]
fn malformed_panel_settings_preserve_other_preferences() {
    for json in [
        r#"{"docking":{"layout":{"root":null,"floating":[]},"hidden":[]},"default_mode":"edit","flatten_fill_sign":true}"#,
        r#"{"side_panels":"not a layout","default_mode":"edit","flatten_fill_sign":true}"#,
    ] {
        let (settings, _) = import_json(json);
        let (mut h, c) = harness();
        let default_dock = dock(&mut h, &c);
        h.state_mut().apply_settings(&settings);
        h.run_steps(3);
        assert_eq!(dock(&mut h, &c), default_dock, "{json}");
        let saved = h.state().settings_snapshot();
        assert_eq!(saved.layout.default_mode, Some(pdfcraft_ui_egui::Mode::Edit), "{json}");
        assert_eq!(saved.behavior.flatten_fill_sign, Some(true), "{json}");
    }
}

#[test]
fn rail_opens_a_panel_then_closes_the_open_one() {
    let (mut h, c) = harness();
    let document = ok(&mut h, &c, "ui.state", json!({}))["documents"].clone();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(4);
    h.get_by_label("This document has no bookmarks.");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(4);
    assert_eq!(dock(&mut h, &c)["side_open"], false);
    assert!(h.query_by_label("This document has no bookmarks.").is_none());
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["documents"], document);
}

#[test]
fn control_quit_retains_inactive_fill_sign_typing_after_cancel() {
    let (mut h, c) = harness();
    h.state_mut().views[0].session.fill_text = Some(pdfcraft_ui_egui::fill_sign::TypeBox {
        page: 0,
        at: [40.0, 200.0],
        text: "Unsaved signature note".into(),
        size: 10.0,
        color: [0.0; 3],
        border: pdfcraft_ui_egui::fill_sign::TextBorder::NONE,
        focus: false,
    });
    h.state_mut().active = None;
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    assert_eq!(ok(&mut h, &c, "ui.quit", json!({})), json!({"quitting":false,"needs_confirmation":true}));
    assert!(!close_requested(&h));
    h.run_steps(3);
    ok(&mut h, &c, "ui.click", json!({"label":"Cancel"}));
    assert_eq!(h.state().views[0].session.fill_text.as_ref().unwrap().text, "Unsaved signature note");
    assert_eq!(h.state().views.len(), 1);
    assert!(!close_requested(&h));
}

#[test]
fn control_quit_retains_blocked_content_typing_after_cancel() {
    if crate::support::skip_without_glyphs("control_quit_retains_blocked_content_typing_after_cancel", "en", "Keep 世界") {
        return;
    }
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.command", json!({"id":"edit.text"}));
    h.run_steps(3);
    let point = h.state().views[0].page_screen_rect(0).unwrap().center();
    ok(&mut h, &c, "ui.click", json!({"x":point.x,"y":point.y}));
    h.event(egui::Event::Text("Keep 世界".into()));
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.content.draft.as_ref().unwrap().text, "Keep 世界");
    h.state_mut().active = None;
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    assert_eq!(ok(&mut h, &c, "ui.quit", json!({})), json!({"quitting":false,"needs_confirmation":true}));
    h.run_steps(3);
    ok(&mut h, &c, "ui.click", json!({"label":"Cancel"}));
    assert_eq!(h.state().views[0].session.content.draft.as_ref().unwrap().text, "Keep 世界");
    assert_eq!(h.state().views.len(), 1);
    assert!(!close_requested(&h));
}

#[test]
fn read_mode_retains_existing_rail_click_to_close_policy() {
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key":"panel", "value":"bookmarks"}));
    ok(&mut h, &c, "ui.set", json!({"key":"mode", "value":"read"}));
    h.run_steps(3);
    assert_eq!(dock(&mut h, &c)["side_panel"], "Bookmarks");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(3);
    assert_eq!(dock(&mut h, &c)["side_open"], false);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
}

#[test]
fn control_open_of_an_already_open_file_replies_ok_and_focuses_its_tab() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.pdf").to_string_lossy().into_owned();
    let b = dir.join("b.pdf").to_string_lossy().into_owned();
    std::fs::write(&a, fixture(2)).unwrap();
    std::fs::write(&b, fixture(3)).unwrap();
    let (mut app, jobs) = app_with_manual_jobs();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app
        }
    });
    h.run_steps(4);
    let c = tx;
    // Opens go through the open queue: the first two are accepted, then open once their jobs run (#1620).
    let first = call_running(&mut h, &c, &jobs, "ui.open", json!({ "path": a })).unwrap();
    assert_eq!(first["accepted"], json!(a), "{first}");
    call_running(&mut h, &c, &jobs, "ui.open", json!({ "path": b })).unwrap();
    run_jobs(&mut h, &jobs);
    assert_eq!(h.state().views.len(), 2, "both files are open");
    // The file is already open: the tab focuses, and the reply must be Ok — an agent driving the
    // app headlessly would otherwise read a focus as a read failure (#1620).
    let again = call_running(&mut h, &c, &jobs, "ui.open", json!({ "path": a })).unwrap();
    assert_eq!(again["focused"], true, "{again}");

    let state = ok(&mut h, &c, "ui.state", json!({}));
    let docs = state["documents"].as_array().cloned().unwrap_or_default();
    assert_eq!(docs.len(), 2, "opening an open file adds no tab: {state}");
    let a_tab = docs.iter().find(|d| d["path"] == json!(a)).unwrap();
    assert_eq!(a_tab["active"], true, "the already-open tab is in front: {state}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other half of the #1620 contract on a live UI: a fresh open is scheduled on the
/// background worker and answered `pending` (upstream's reply shape), the tab appears once
/// the open completes, and opening the file again then focuses that tab.
#[test]
fn control_open_replies_pending_while_the_worker_opens_and_focuses_once_it_has() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-open-pending-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("pending.pdf").to_string_lossy().into_owned();
    std::fs::write(&a, fixture(2)).unwrap();
    let (mut app, jobs) = app_with_manual_jobs();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app
        }
    });
    h.run_steps(4);
    let c = tx;

    // The host holds the open job back, so the reply can only be `pending`.
    let first = ok(&mut h, &c, "ui.open", json!({ "path": a }));
    assert_eq!(first["accepted"], json!(a), "{first}");
    assert_eq!(first["pending"], true, "{first}");
    let tabs = |h: &mut Harness<'static, PdfCraftApp>| ok(h, &c, "ui.state", json!({}))["documents"].as_array().map(|d| d.len()).unwrap_or_default();
    assert_eq!(tabs(&mut h), 0, "nothing opens until the job runs");

    // The job completes the open on later frames; the tab shows, and only then does a repeated
    // open focus it instead of reporting another pending open.
    run_jobs(&mut h, &jobs);
    assert_eq!(tabs(&mut h), 1, "the pending open completed into a tab");

    let again = ok(&mut h, &c, "ui.open", json!({ "path": a }));
    assert_eq!(again["focused"], true, "{again}");
    let state = ok(&mut h, &c, "ui.state", json!({}));
    let docs = state["documents"].as_array().cloned().unwrap_or_default();
    assert_eq!(docs.len(), 1, "focusing the completed open adds no tab: {state}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn navigation_frame(h: &mut Harness<'static, PdfCraftApp>, events: Vec<egui::Event>) {
    h.input_mut().events.extend(events);
    h.step();
}

fn navigation_wheel(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2, delta: f32, ctrl: bool) {
    let modifiers = egui::Modifiers { ctrl, command: ctrl, ..egui::Modifiers::NONE };
    navigation_frame(
        h,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                modifiers,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            },
        ],
    );
    h.run_steps(20);
    navigation_frame(h, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
}

#[test]
fn ctrl_wheel_anchors_visible_page_and_plain_wheel_only_scrolls() {
    for percent in [100, 200, 400] {
        for fraction in [0.1, 0.5, 0.9] {
            let (mut h, c) = harness_pages(20);
            ok(&mut h, &c, "ui.set", json!({"key":"zoom", "value":percent.to_string()}));
            h.run_steps(4);
            let viewport = h.state().views[0].viewport_rect();
            let before = h.state().views[0].page_screen_rect(0).unwrap();
            let visible = before.intersect(viewport).shrink(15.0);
            let p = egui::pos2(visible.left() + fraction * visible.width(), visible.top() + fraction * visible.height());
            let normalised = (p - before.min) / before.size();
            let old_zoom = h.state().views[0].zoom();
            navigation_wheel(&mut h, p, 20.0, true);
            let enlarged = h.state().views[0].page_screen_rect(0).unwrap();
            let anchored = enlarged.min + normalised * enlarged.size();
            assert!(h.state().views[0].zoom() > old_zoom);
            assert!((anchored.y - p.y).abs() < 2.0, "percent={percent}, fraction={fraction}");
            if before.width() > viewport.width() {
                assert!((anchored.x - p.x).abs() < 2.0);
            }
            navigation_wheel(&mut h, p, -20.0, true);
            assert!((h.state().views[0].zoom() - old_zoom).abs() < 0.01);
            let zoom = h.state().views[0].zoom();
            let top = h.state().views[0].page_screen_rect(0).unwrap().top();
            navigation_wheel(&mut h, p, -80.0, false);
            assert_eq!(h.state().views[0].zoom(), zoom);
            assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0);
        }
    }
}

#[test]
fn wheel_zoom_is_scoped_to_the_viewport_and_respects_existing_limits() {
    let (mut h, c) = harness();
    let outside = egui::pos2(5.0, 5.0);
    let zoom = h.state().views[0].zoom();
    navigation_wheel(&mut h, outside, 40.0, true);
    assert_eq!(h.state().views[0].zoom(), zoom);
    for (percent, delta, expected) in [(6400, 80.0, 64.0), (8, -80.0, 0.08)] {
        ok(&mut h, &c, "ui.set", json!({"key":"zoom", "value":percent.to_string()}));
        h.run_steps(4);
        let p = h.state().views[0].viewport_rect().center();
        navigation_wheel(&mut h, p, delta, true);
        assert_eq!(h.state().views[0].zoom(), expected);
    }
}

#[test]
fn initial_zoom_option_is_applied_before_the_first_viewport_layout() {
    for percent in [50, 100, 200, 400] {
        let h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = new_app();
                app.open_bytes(&egui::Context::default(), "initial-zoom.pdf", None, fixture(3)).unwrap();
                app.set_option(&egui::Context::default(), "zoom", &percent.to_string()).unwrap();
                assert_eq!(app.views[0].zoom(), f64::from(percent) / 100.0, "initial options must work before the viewport exists");
                app
            }
        });
        assert_eq!(h.state().views[0].zoom(), f64::from(percent) / 100.0);
    }
}

#[test]
fn page_size_shows_while_the_pointer_is_in_the_bottom_left_quadrant() {
    let (mut h, _c) = harness();
    let area = h.state().views[0].viewport_rect();
    let shown = |h: &Harness<'static, PdfCraftApp>| {
        let size = {
            let s = h.state();
            let p = &s.session.get(s.views[0].id).unwrap().info.pages[0];
            s.units.size(f64::from(p.width), f64::from(p.height))
        };
        h.query_by_label(&size).is_some()
    };
    let away = area.center() + egui::vec2(300.0, -200.0);
    h.event(egui::Event::PointerMoved(away));
    h.run_steps(3);
    assert!(!shown(&h), "top-right: hidden");
    h.event(egui::Event::PointerMoved(area.left_bottom() + egui::vec2(80.0, -80.0)));
    h.run_steps(3);
    assert!(shown(&h), "bottom-left: shown");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOT_DIR") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/page-size-hint.png")).unwrap();
    }
    h.event(egui::Event::PointerMoved(away));
    h.run_steps(3);
    assert!(!shown(&h), "pointer left: hidden again");
}

#[test]
fn navigation_history_commands_and_rail_are_per_tab_and_non_editing() {
    for os in [egui::os::OperatingSystem::Nix, egui::os::OperatingSystem::Mac, egui::os::OperatingSystem::Windows] {
        navigation_history_for_os(os);
    }
}

fn navigation_history_for_os(os: egui::os::OperatingSystem) {
    let (mut h, c) = harness();
    h.ctx.set_os(os);
    h.run_steps(2);
    let enabled =
        |list: Value, id: &str| list["commands"].as_array().unwrap().iter().find(|cmd| cmd["id"] == id).unwrap()["enabled"].as_bool().unwrap();
    assert!(!enabled(ok(&mut h, &c, "ui.commands", json!({})), "view.previous_view"));
    assert!(call(&mut h, &c, "ui.command", json!({"id": "view.previous_view"})).is_err());
    // Empty-history shortcuts were silent before registry integration and must stay silent.
    for key in ["[", "]"] {
        ok(&mut h, &c, "ui.key", json!({"key": key, "modifiers": {"command": true}}));
        assert!(crate::support::shown_notice(&h).is_none(), "an unavailable history action must not ask to open the already-open document");
    }
    let before = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    ok(&mut h, &c, "ui.set", json!({"key": "page", "value": "4"}));
    assert!(enabled(ok(&mut h, &c, "ui.commands", json!({})), "view.previous_view"));
    // The real rail button is accessible and invokes the same history, not the adjacent page.
    // The toolbar names its slots by their commands (L-233).
    ok(&mut h, &c, "ui.click", json!({"label": "Previous view"}));
    // `IconButton::command` queues `view.previous_view` on the command board during the click's pass;
    // the app drains fired commands in the next frame's `prepare_raw_input`.
    h.step();
    assert_eq!(h.state().views[0].current(), 0);
    assert!(enabled(ok(&mut h, &c, "ui.commands", json!({})), "view.next_view"));
    ok(&mut h, &c, "ui.command", json!({"id": "view.next_view"}));
    assert_eq!(h.state().views[0].current(), 3);
    ok(&mut h, &c, "ui.set", json!({"key": "page", "value": "5"}));
    ok(&mut h, &c, "ui.key", json!({"key": "[", "modifiers": {"command": true}}));
    assert_eq!(h.state().views[0].current(), 3, "shortcut traverses exactly one history entry");
    ok(&mut h, &c, "ui.key", json!({"key": "]", "modifiers": {"command": true}}));
    assert_eq!(h.state().views[0].current(), 4);
    ok(&mut h, &c, "ui.command", json!({"id": "view.previous_view"}));
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, fixture(5)).unwrap();
    assert!(!enabled(ok(&mut h, &c, "ui.commands", json!({})), "view.previous_view"));
    ok(&mut h, &c, "ui.set", json!({"key": "page", "value": "2"}));
    ok(&mut h, &c, "ui.command", json!({"id": "view.previous_view"}));
    assert_eq!(h.state().views[1].current(), 0);
    assert_eq!(h.state().views[0].current(), 3, "other tab's history does not move this document");
    h.state_mut().active = Some(0);
    ok(&mut h, &c, "ui.command", json!({"id": "view.previous_view"}));
    assert_eq!(h.state().views[0].current(), 0);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, before);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "reopened.pdf", None, before.as_ref().clone()).unwrap();
    assert!(h.state().views.last().unwrap().back.is_empty());
    assert!(h.state().views.last().unwrap().forward.is_empty());
}

#[test]
fn routine_continuous_scrolling_does_not_add_navigation_history() {
    let (mut h, c) = harness_pages(20);
    ok(&mut h, &c, "ui.set", json!({"key": "animation_effects", "value": "off"}));
    h.hover_at(egui::pos2(700.0, 450.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -1200.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(8);
    assert!(h.state().views[0].current() > 0, "the wheel really moved to a later page");
    assert!(h.state().views[0].back.is_empty());
    assert!(h.state().views[0].forward.is_empty());
}

/// CodeRabbit on #5341: the control channel folds a pinned folder on Home down to its heading
/// and opens it again, as its chevron does, and `ui.state` says which are folded. Folders that
/// aren't pinned, and states other than on and off, are refused with the reason.
#[test]
fn control_collapses_and_expands_a_pinned_folder() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("inside.pdf"), fixture(1)).unwrap();
    let folder = dir.to_string_lossy().into_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut app, jobs) = app_with_manual_jobs();
    let pin = folder.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.pin_folder(&pin);
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app
        }
    });
    let c = tx;
    // The listing of the pinned folder is a job; the test runs it between frames.
    run_jobs(&mut h, &jobs);
    let folded = |h: &mut Harness<'static, PdfCraftApp>| ok(h, &c, "ui.state", json!({}))["pinned_folders"].clone();
    assert_eq!(folded(&mut h), json!([{ "folder": folder, "collapsed": false }]));
    // A file's row is named by the file, then when it changed and its size.
    h.get_by_label_contains("inside.pdf");
    ok(&mut h, &c, "ui.set", json!({ "key": "folder-collapsed", "value": format!("{folder}=on") }));
    run_jobs(&mut h, &jobs);
    assert_eq!(folded(&mut h), json!([{ "folder": folder, "collapsed": true }]));
    assert!(h.query_by_label_contains("inside.pdf").is_none(), "a collapsed folder lists nothing");
    let unpinned = call(&mut h, &c, "ui.set", json!({ "key": "folder-collapsed", "value": "/not/pinned=on" })).unwrap_err();
    assert!(unpinned.contains("not a pinned folder"), "{unpinned}");
    let bad = call(&mut h, &c, "ui.set", json!({ "key": "folder-collapsed", "value": format!("{folder}=yes") })).unwrap_err();
    assert!(bad.contains("=on or"), "{bad}");
    assert_eq!(folded(&mut h), json!([{ "folder": folder, "collapsed": true }]), "refusals change nothing");
    ok(&mut h, &c, "ui.set", json!({ "key": "folder-collapsed", "value": format!("{folder}=off") }));
    run_jobs(&mut h, &jobs);
    assert_eq!(folded(&mut h), json!([{ "folder": folder, "collapsed": false }]));
    h.get_by_label_contains("inside.pdf");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn accessible_labels_on_format_text_and_pin_dialog() {
    let (mut h, c) = harness();

    // Open Add Text so Format text panel is visible
    ok(&mut h, &c, "ui.command", json!({"id": "edit.text"}));
    h.run_steps(3);

    let font_combo = ok(&mut h, &c, "ui.inspect", json!({"query": "Font", "role": "ComboBox"}));
    assert!(font_combo["count"].as_u64().unwrap() >= 1, "format-font has accessible label: {font_combo}");

    let size_combo = ok(&mut h, &c, "ui.inspect", json!({"query": "Font size"}));
    assert!(size_combo["count"].as_u64().unwrap() >= 1, "format-size has accessible label: {size_combo}");

    let line_spacing_combo = ok(&mut h, &c, "ui.inspect", json!({"query": "Line spacing", "role": "ComboBox"}));
    assert!(line_spacing_combo["count"].as_u64().unwrap() >= 1, "format-line-spacing has accessible label: {line_spacing_combo}");

    let scale_spin = ok(&mut h, &c, "ui.inspect", json!({"query": "Horizontal scale", "role": "SpinButton"}));
    assert!(scale_spin["count"].as_u64().unwrap() >= 1, "horizontal scale spin button has accessible label: {scale_spin}");

    let char_spin = ok(&mut h, &c, "ui.inspect", json!({"query": "Character spacing", "role": "SpinButton"}));
    assert!(char_spin["count"].as_u64().unwrap() >= 1, "character spacing spin button has accessible label: {char_spin}");
}
