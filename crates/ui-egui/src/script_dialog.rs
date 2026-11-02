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
    pub(crate) fn start_ui_script(&mut self, id: DocId, script: &str, field: Option<&str>) {
        if self.script_pending.is_some() {
            return;
        }
        let Some(view) = self.views.iter().find(|v| v.id == id) else { return };
        let (zoom, page, highlight) = (view.zoom, view.current, view.highlight_fields);
        self.session.set_javascript_view(id, zoom as f64 * 100.0, highlight, page);
        match self.session.start_javascript(id, script, field) {
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
        let current = self.active_ids().is_some_and(|(_, id)| id == pending.run.id)
            && self.session.script_pending_current(&pending.run)
            && self.views.iter().find(|v| v.id == pending.run.id).is_some_and(|v| v.zoom == pending.zoom && v.current == pending.page);
        if !current
            || (pending.prompt.is_none() && pending.started.elapsed() > Duration::from_secs(10))
            || (pending.prompt.is_some() && pending.started.elapsed() > Duration::from_secs(300))
        {
            self.session.invalidate_javascript(pending.run.id);
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
                    self.session.invalidate_javascript(pending.run.id);
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
        let modal = crate::widgets::modal(ctx, ("document-script-prompt", pending.run.id.0, pending.serial)).show(ctx, |ui| {
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
        app.start_ui_script(id, "app.response({cQuestion:'Wait'});", None);
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
