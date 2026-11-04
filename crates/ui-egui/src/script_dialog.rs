//! Script prompts suspend the document worker, never the egui event loop.
use crate::PdfCraftApp;
use pdfcraft_engine::{
    DocId,
    js::{InteractiveEvent, JsOutput, MenuItem, PendingScript, Prompt},
};
use std::sync::mpsc::{Sender, TryRecvError};
use std::time::{Duration, Instant};

pub(crate) struct Pending {
    run: PendingScript,
    prompt: Option<(Prompt, Sender<Option<String>>)>,
    text: String,
    focus: bool,
    started: Instant,
    zoom: f32,
    page: usize,
    serial: u32,
}
impl Pending {
    /// Whether this invocation is waiting for an explicit user reply rather than worker execution.
    pub(crate) fn has_prompt(&self) -> bool {
        self.prompt.is_some()
    }
}

/// Draw the bounded menu tree and return only the selected enabled leaf value.
fn menu(ui: &mut egui::Ui, items: &[MenuItem], answer: &mut Option<String>) {
    for item in items {
        if item.name == "-" {
            ui.separator();
            continue;
        }
        ui.add_enabled_ui(item.enabled, |ui| {
            if item.children.is_empty() {
                if ui.selectable_label(item.marked, &item.name).clicked() {
                    *answer = Some(item.value.clone());
                }
            } else {
                egui::CollapsingHeader::new(&item.name).show(ui, |ui| menu(ui, &item.children, answer));
            }
        });
    }
}
impl PdfCraftApp {
    /// Start one explicit button interaction with captured document/view state and leave egui responsive.
    pub(crate) fn start_ui_script(&mut self, id: DocId, script: &str, field: Option<&str>, identity: Option<u64>) {
        if self.script_pending.is_some() {
            self.notify("Another form script is active; this click was not run");
            return;
        }
        let Some(view) = self.views.iter().find(|v| v.id == id) else { return };
        let (zoom, page, highlight) = (view.zoom, view.current, view.highlight_fields);
        self.session.set_javascript_view(id, zoom as f64 * 100.0, highlight, page);
        match self.session.start_javascript_target(id, script, field, identity) {
            Ok(run) => {
                self.script_pending =
                    Some(Pending { run, prompt: None, text: String::new(), focus: false, started: Instant::now(), zoom, page, serial: 0 })
            }
            Err(e) => self.notify(e.to_string()),
        }
    }
    /// Receive worker prompts/results and invalidate stale or timed-out interactions before applying their edits.
    pub(crate) fn poll_script_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.script_pending.take() else { return };
        let current = (self.active_ids().is_some_and(|(_, id)| id == pending.run.id)
            || self.compare.as_ref().is_some_and(|c| {
                c.side_by_side
                    && (c.old == pending.run.id || c.new == pending.run.id)
                    && self.active_ids().is_some_and(|(_, active)| active == c.old || active == c.new)
            }))
            && self.session.script_pending_current(&pending.run)
            && self.views.iter().find(|v| v.id == pending.run.id).is_some_and(|v| v.zoom == pending.zoom && v.current == pending.page);
        if !current {
            self.session.discard_javascript(&pending.run);
            return;
        }
        if (pending.prompt.is_none() && pending.started.elapsed() > Duration::from_secs(10))
            || (pending.prompt.is_some() && pending.started.elapsed() > Duration::from_secs(300))
        {
            self.session.discard_javascript(&pending.run);
            self.notify("The form script timed out; its result was discarded");
            return;
        }
        if pending.prompt.is_none() {
            match pending.run.receiver.try_recv() {
                Ok(InteractiveEvent::Prompt(prompt, reply)) => {
                    pending.text = match &prompt {
                        Prompt::Text { default, .. } => default.clone(),
                        _ => String::new(),
                    };
                    pending.serial = pending.serial.saturating_add(1);
                    pending.focus = true;
                    pending.started = Instant::now();
                    pending.prompt = Some((prompt, reply));
                }
                Ok(InteractiveEvent::Complete(out)) => {
                    let id = pending.run.id;
                    match self.session.finish_javascript(pending.run, out) {
                        Ok(out) => {
                            if let Some(i) = self.views.iter().position(|v| v.id == id)
                                && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                            {
                                self.views[i].document_changed(&info);
                            }
                            self.handle_js(
                                id,
                                JsOutput {
                                    alerts: Vec::new(),
                                    console: out.console,
                                    requests: out.requests,
                                    errors: out.error.into_iter().chain(out.doc_errors).collect(),
                                    doc_errors: Vec::new(),
                                },
                            );
                        }
                        Err(e) => self.notify(e.to_string()),
                    }
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.session.discard_javascript(&pending.run);
                    self.notify("The form script stopped unexpectedly");
                    return;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        // Input events wake egui immediately; idle prompts only need a coarse timeout check.
        ctx.request_repaint_after(if pending.has_prompt() { Duration::from_secs(1) } else { Duration::from_millis(16) });
        self.script_pending = Some(pending);
    }
    /// Display the current typed prompt and send one answer/cancellation without replaying script side effects.
    pub(crate) fn show_script_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.script_pending.as_mut() else { return };
        let Some((prompt, _)) = pending.prompt.as_ref() else { return };
        let mut answer = None;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let prompt_modal = crate::widgets::modal(ctx, ("document-script-prompt", pending.run.id.0, pending.serial));
        // A different dialog may have been opened after this prompt. Drawing last alone
        // does not reorder retained egui areas, so explicitly retain modal input priority.
        ctx.move_to_top(prompt_modal.area.layer());
        let modal = prompt_modal.show(ctx, |ui| {
            ui.set_width(360.0);
            match prompt {
                Prompt::Menu(items) => {
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| menu(ui, items, &mut answer));
                }
                Prompt::Text { title, question, password, label, .. } => {
                    ui.heading(title);
                    ui.label(question);
                    if !label.is_empty() {
                        ui.label(label);
                    }
                    let field = ui.add(egui::TextEdit::singleline(&mut pending.text).password(*password).char_limit(4096));
                    if pending.focus {
                        field.request_focus();
                        pending.focus = false;
                    }
                    if ui.add_enabled(pending.text.len() <= 4096, egui::Button::new(tl!("OK"))).clicked() {
                        answer = Some(pending.text.clone());
                    }
                }
                Prompt::Alert(message) => {
                    if let Some(title) = &message.title {
                        ui.heading(title);
                    }
                    ui.label(&message.text);
                    use pdfcraft_engine::js::MessageButtons as B;
                    ui.horizontal(|ui| match message.buttons {
                        B::Ok | B::OkCancel => {
                            if ui.button(tl!("OK")).clicked() {
                                answer = Some("1".into());
                            }
                        }
                        B::YesNo | B::YesNoCancel => {
                            if ui.button(tl!("Yes")).clicked() {
                                answer = Some("4".into());
                            }
                            if ui.button(tl!("No")).clicked() {
                                answer = Some("3".into());
                            }
                        }
                    });
                }
            }
            if ui.button(tl!("Cancel")).clicked() {
                cancel = true;
            }
        });
        cancel |= answer.is_none() && modal.should_close();
        if answer.is_some() || cancel {
            if let Some((prompt, reply)) = pending.prompt.take() {
                let answer = if cancel {
                    match prompt {
                        Prompt::Alert(m) => Some(
                            match m.buttons {
                                pdfcraft_engine::js::MessageButtons::Ok => "1",
                                pdfcraft_engine::js::MessageButtons::YesNo => "3",
                                _ => "2",
                            }
                            .into(),
                        ),
                        _ => None,
                    }
                } else {
                    answer
                };
                let _ = reply.send(answer);
            }
            pending.started = Instant::now();
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    /// The central shortcut/page-input gate must see a real pending script prompt.
    #[test]
    fn a_script_prompt_blocks_document_input_until_answered() {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        app.start_ui_script(id, "app.response({cQuestion:'Wait'});", None, None);
        let ctx = egui::Context::default();
        for _ in 0..300 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.as_ref().is_some_and(|p| p.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.script_pending.as_ref().unwrap().prompt.is_some());
        assert!(app.modal_open(), "an unanswered script prompt must block document input");
        let (_, reply) = app.script_pending.as_mut().unwrap().prompt.take().unwrap();
        reply.send(None).unwrap();
        for _ in 0..300 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.script_pending.is_none());
        assert!(!app.modal_open());
    }
}

#[cfg(test)]
mod review13_ui_tests {
    use super::*;
    /// Create a native app with a visible synthetic blank document and no renderer.
    fn app() -> (PdfCraftApp, DocId) {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(200.0, 300.0, 2).unwrap();
        app.open_bytes("synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        (app, id)
    }
    /// Refused clicks and execution/prompt timeouts are visible, while stale runs stay silent.
    #[test]
    fn busy_and_timeout_notices_do_not_claim_worker_termination() {
        let ctx = egui::Context::default();
        for (timeout, prompt) in [(true, false), (true, true), (false, false)] {
            let (mut app, id) = app();
            app.start_ui_script(id, "app.response({cQuestion:'Wait'});", None, None);
            app.start_ui_script(id, "var shouldNotRun=1;", None, None);
            assert!(app.toast.as_ref().unwrap().0.contains("not run"));
            app.toast = None;
            if prompt {
                let deadline = Instant::now() + Duration::from_secs(5);
                while app.script_pending.as_ref().unwrap().prompt.is_none() && Instant::now() < deadline {
                    app.poll_script_dialog(&ctx);
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(app.script_pending.as_ref().unwrap().prompt.is_some());
            }
            if timeout {
                app.script_pending.as_mut().unwrap().started = Instant::now() - Duration::from_secs(if prompt { 301 } else { 11 });
            } else {
                app.views.iter_mut().find(|v| v.id == id).unwrap().zoom += 1.0;
            }
            app.poll_script_dialog(&ctx);
            assert!(app.script_pending.is_none());
            if timeout {
                assert_eq!(app.toast.as_ref().unwrap().0, "The form script timed out; its result was discarded");
            } else {
                assert!(app.toast.is_none());
            }
        }
    }
    /// The visible old comparison half may complete without being the active new-file tab.
    #[test]
    fn non_active_comparison_document_completes_native_script() {
        let (mut app, old) = app();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("new.pdf", None, bytes.to_vec()).unwrap();
        assert_ne!(app.active_ids().unwrap().1, old);
        app.compare_old = Some(old);
        app.run_compare();
        assert!(app.compare.as_ref().unwrap().side_by_side);
        app.start_ui_script(old, "var comparisonKept=42;", None, None);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.script_pending.is_some() && Instant::now() < deadline {
            app.poll_script_dialog(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.script_pending.is_none());
        assert_eq!(app.session.run_javascript(old, "comparisonKept", None).unwrap().result.as_deref(), Some("42"));
    }
    /// Script focus reveals the requested field page before opening its editor.
    #[test]
    fn script_focus_reveals_an_offscreen_widget() {
        let (mut app, id) = app();
        app.session
            .apply(
                id,
                pdfcraft_engine::Edit::AddField {
                    page: 1,
                    rect: [20.0, 20.0, 120.0, 40.0],
                    kind: pdfcraft_engine::NewField::Text { multiline: false },
                    name: Some("later".into()),
                },
            )
            .unwrap();
        app.handle_js(id, JsOutput { requests: vec![pdfcraft_engine::js::Request::Focus("later".into())], ..Default::default() });
        let view = app.views.iter().find(|v| v.id == id).unwrap();
        assert_eq!(view.current, 1);
        assert_eq!(view.forms.focus.as_ref().unwrap().name, "later");
    }
}
