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
    /// Link-click position and view, retained while the worker reaches its menu.
    link_anchor: Option<(u64, egui::Pos2)>,
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

/// CAD properties are flat key/value rows; other menus retain the interactive upstream UI.
fn component_properties(items: &[MenuItem]) -> bool {
    !items.is_empty()
        && items.iter().any(|item| item.name.contains(": "))
        && items.iter().all(|item| {
            item.enabled && !item.marked && item.children.is_empty() && item.value == item.name && (item.name == "-" || item.name.contains(": "))
        })
}

/// Display CAD properties beside the clicked component, within its document viewport.
fn component_popup(ctx: &egui::Context, anchor: egui::Pos2, viewport: egui::Rect, items: &[MenuItem]) -> std::task::Poll<Option<String>> {
    use std::task::Poll;
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return Poll::Ready(None);
    }
    let bounds = viewport.intersect(ctx.content_rect()).shrink(8.0);
    if !bounds.is_positive() {
        return Poll::Pending;
    }
    let t = crate::theme::Tokens::get(ctx);
    let mut open = true;
    let mut answer = None;
    let popup = egui::Area::new(egui::Id::unique("popup-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor + egui::vec2(12.0, 12.0))
        .constrain_to(bounds)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.panel)
                .stroke(egui::Stroke::new(1.0, t.divider))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .shadow(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: egui::Color32::from_black_alpha(40) })
                .show(ui, |ui| {
                    ui.set_width(360.0_f32.min((bounds.width() - 24.0).max(0.0)));
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tl!("Component Properties")).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::icons::button(ui, "x", 24.0, false, tl!("Close (Esc)")).clicked() {
                                open = false;
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .max_height((bounds.height() - 80.0).max(0.0).min(bounds.height() * 0.6))
                        .show(ui, |ui| {
                            for item in items {
                                if item.name == "-" {
                                    ui.separator();
                                } else if let Some((key, value)) = item.name.split_once(": ") {
                                    let row = ui.horizontal_wrapped(|ui| {
                                        ui.label(egui::RichText::new(key).strong());
                                        ui.label(value);
                                    });
                                    if item.enabled && row.response.interact(egui::Sense::click()).clicked() {
                                        answer = Some(item.value.clone());
                                    }
                                }
                            }
                        });
                });
        });
    let outside_click = ctx.input(|input| {
        input.pointer.button_pressed(egui::PointerButton::Primary)
            && input.pointer.interact_pos().is_some_and(|pos| viewport.contains(pos) && !popup.response.rect.contains(pos))
    });
    if !open || outside_click {
        Poll::Ready(None)
    } else if answer.is_some() {
        Poll::Ready(answer)
    } else {
        Poll::Pending
    }
}
/// Consume the current prompt before replying so two UI events cannot resume it twice.
fn reply_to_prompt(ctx: &egui::Context, pending: &mut Pending, answer: Option<String>, cancel: bool) {
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

impl PdfCraftApp {
    /// Start one explicit button interaction with captured document/view state and leave egui responsive.
    pub(crate) fn start_ui_script(&mut self, id: DocId, script: &str, field: Option<&str>, identity: Option<u64>) {
        self.start_script_with(id, false, |s| s.start_javascript_target(id, script, field, identity));
    }
    /// Start link `index` on page `page`'s JavaScript action (0-based) as a button's runs: its
    /// menus and questions are asked without blocking the window (#2081).
    pub(crate) fn start_link_script(&mut self, id: DocId, page: usize, index: usize) {
        self.start_script_with(id, true, |s| s.start_link_javascript(id, page, index));
    }
    fn start_script_with(
        &mut self,
        id: DocId,
        link: bool,
        start: impl FnOnce(&mut pdfcraft_engine::Session) -> Result<PendingScript, pdfcraft_engine::EditError>,
    ) {
        if self.script_pending.is_some() {
            self.notify("Another form script is active; this click was not run");
            return;
        }
        let Some(view) = self.views.iter().find(|v| v.id == id) else { return };
        let (zoom, page, highlight) = (view.zoom, view.current, view.highlight_fields);
        let pointer = self.ctx.as_ref().and_then(|ctx| ctx.input(|i| i.pointer.interact_pos()));
        let link_anchor = link.then(|| {
            let view = pointer.and_then(|p| self.views.iter().find(|v| v.id == id && v.document_viewport().contains(p))).unwrap_or(view);
            (view.view_key(), pointer.unwrap_or_else(|| view.document_viewport().center()))
        });
        self.session.set_javascript_view(id, zoom as f64 * 100.0, highlight, page);
        match start(&mut self.session) {
            Ok(run) => {
                self.script_pending = Some(Pending {
                    run,
                    prompt: None,
                    text: String::new(),
                    focus: false,
                    started: Instant::now(),
                    zoom,
                    page,
                    serial: 0,
                    link_anchor,
                })
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
            self.session.timeout_javascript(pending.run);
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
                    let before = self.session.get(id).map(|doc| doc.edit_generation());
                    match self.session.finish_javascript(pending.run, out) {
                        Ok(out) => {
                            if let Some(doc) = self.session.get(id) {
                                if Some(doc.edit_generation()) != before {
                                    for view in self.views.iter_mut().filter(|view| view.id == id) {
                                        view.document_changed(&doc.info);
                                    }
                                } else if !out.layers.is_empty() {
                                    for view in self.views.iter_mut().filter(|view| view.id == id) {
                                        view.invalidate_content();
                                    }
                                }
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
        let placement = self
            .script_pending
            .as_ref()
            .and_then(|p| p.link_anchor)
            .and_then(|(key, anchor)| self.views.iter().find(|v| v.view_key() == key).map(|v| (anchor, v.document_viewport())));
        self.show_script_dialog_with_menu(ctx, |ctx, _, items| {
            let (anchor, viewport) = placement?;
            if !component_properties(items) {
                return None;
            }
            Some(component_popup(ctx, anchor, viewport, items))
        });
    }

    /// Let an alternative renderer own a menu until it returns a selection or cancellation.
    /// Returning `None` uses the built-in modal; the renderer never receives the reply sender.
    pub fn show_script_dialog_with_menu(
        &mut self,
        ctx: &egui::Context,
        render_menu: impl FnOnce(&egui::Context, DocId, &[MenuItem]) -> Option<std::task::Poll<Option<String>>>,
    ) {
        let Some(pending) = self.script_pending.as_mut() else { return };
        let Some((prompt, _)) = pending.prompt.as_ref() else { return };
        let Some(doc) = self.session.get(pending.run.id) else { return };
        if let Prompt::Menu(items) = prompt
            && let Some(reply) = render_menu(ctx, pending.run.id, items)
        {
            if let std::task::Poll::Ready(answer) = reply {
                let cancel = answer.is_none();
                reply_to_prompt(ctx, pending, answer, cancel);
            }
            return;
        }
        // Keep the filename in a separate, single-line label. It is document data too.
        let document_name: String = doc
            .name
            .chars()
            .filter(|c| !c.is_control() && !matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
            .take(160)
            .collect();
        let mut answer = None;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let prompt_modal = crate::widgets::modal(ctx, ("document-script-prompt", pending.run.id.0, pending.serial));
        // A different dialog may have been opened after this prompt. Drawing last alone
        // does not reorder retained egui areas, so explicitly retain modal input priority.
        ctx.move_to_top(prompt_modal.area.layer());
        let modal = prompt_modal.show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(tl!("This document's script is asking"));
            ui.add(egui::Label::new(crate::i18n::fmt(tl!("Document: {name}"), &[("name", &document_name)])).truncate());
            ui.separator();
            match prompt {
                Prompt::Menu(items) => {
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| menu(ui, items, &mut answer));
                }
                Prompt::Text { title, question, password, label, .. } => {
                    if *password {
                        ui.label(tl!("This request comes from the document, not PdfCraft. Only enter a password if you trust this document."));
                    }
                    if !title.is_empty() {
                        ui.label(title);
                    }
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
                        ui.label(title);
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
            reply_to_prompt(ctx, pending, answer, cancel);
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
    /// Comparison completion preserves globals and sends Save As/Print to its source document.
    #[test]
    fn non_active_comparison_document_completes_native_script() {
        for request in ["", "app.execMenuItem('SaveAs');", "print();"] {
            let (mut app, old) = app();
            app.views.iter_mut().find(|v| v.id == old).unwrap().current = 1;
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes("new.pdf", None, bytes.to_vec()).unwrap();
            let new = app.active_ids().unwrap().1;
            assert_ne!(new, old);
            let new_before = app.session.save_bytes(new).unwrap();
            app.compare_old = Some(old);
            app.run_compare();
            assert!(app.compare.as_ref().unwrap().side_by_side);
            let dir = std::env::temp_dir().join(format!(
                "pdfcraft-comparison-request-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&dir).unwrap();
            let saved = dir.join("source.pdf");
            app.save_override = Some(saved.to_string_lossy().into_owned());
            app.printer_source = Some(std::sync::Arc::new(Vec::new));
            app.start_ui_script(old, &format!("var comparisonKept=42;{request}"), None, None);
            let ctx = egui::Context::default();
            let deadline = Instant::now() + Duration::from_secs(12);
            while app.script_pending.is_some() && Instant::now() < deadline {
                app.poll_script_dialog(&ctx);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.script_pending.is_none());
            assert_eq!(app.session.run_javascript(old, "comparisonKept", None).unwrap().result.as_deref(), Some("42"));
            assert_eq!(app.session.save_bytes(new).unwrap(), new_before);
            assert!(app.session.get(new).unwrap().path.is_none());
            if request.contains("SaveAs") {
                assert_eq!(app.active_ids().unwrap().1, new, "Save As need not change the comparison tab");
                assert_eq!(app.session.get(old).unwrap().path.as_deref(), saved.to_str());
                let mut reopened = pdfcraft_engine::Session::new();
                let id = reopened.open("saved.pdf", None, std::sync::Arc::new(std::fs::read(&saved).unwrap()), None).unwrap();
                assert_eq!(reopened.get(id).unwrap().info.pages.len(), 2, "the saved file is the two-page source, not the one-page active file");
            } else if request == "print();" {
                assert_eq!(app.active_ids().unwrap().1, old, "Print must retain its source as the active document");
                assert!(matches!(app.dialog, Some(crate::Dialog::Print)));
                assert_eq!(app.print_draft.current_page, 1);
                assert!(!saved.exists());
            } else {
                assert_eq!(app.active_ids().unwrap().1, new);
                assert!(!saved.exists());
            }
            drop(app);
            if saved.exists() {
                std::fs::remove_file(saved).unwrap();
            }
            std::fs::remove_dir(dir).unwrap();
        }
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

#[cfg(test)]
mod link_script_tests {
    use super::*;

    #[test]
    fn flat_component_properties_use_the_popup_and_escape_closes_it() {
        let items = vec![
            MenuItem { name: "Value: 10k".into(), value: "Value: 10k".into(), enabled: true, marked: false, children: Vec::new() },
            MenuItem { name: "Part: R1".into(), value: "Part: R1".into(), enabled: true, marked: false, children: Vec::new() },
        ];
        assert!(component_properties(&items));
        let mut ordinary = items.clone();
        ordinary[0].name = "Open".into();
        assert!(!component_properties(&ordinary));
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let anchor = egui::pos2(100.0, 100.0);
        for _ in 0..3 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(component_popup(ui.ctx(), anchor, viewport, &items).is_pending());
            })
            .drop_without_applying_deltas();
        }
        let popup = ctx.memory(|m| m.area_rect(egui::Id::unique("popup-menu"))).unwrap();
        assert!(popup.min.distance(anchor) < 32.0 && viewport.contains_rect(popup), "{popup:?}");
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            assert!(matches!(component_popup(ui.ctx(), anchor, viewport, &items), std::task::Poll::Ready(None)));
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn component_popup_closes_on_a_canvas_click_outside_it() {
        let items = vec![MenuItem { name: "Value: 10k".into(), value: "Value: 10k".into(), enabled: true, marked: false, children: Vec::new() }];
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let anchor = egui::pos2(100.0, 100.0);
        for _ in 0..3 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(component_popup(ui.ctx(), anchor, viewport, &items).is_pending());
            })
            .drop_without_applying_deltas();
        }
        let pos = egui::pos2(700.0, 500.0);
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ],
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            assert!(matches!(component_popup(ui.ctx(), anchor, viewport, &items), std::task::Poll::Ready(None)));
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn a_script_without_document_changes_keeps_the_view_cache() {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        app.start_ui_script(id, "1 + 1", None, None);
        app.views[0].goto = Some((0, 0.25));
        let ctx = egui::Context::default();
        for _ in 0..400 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.script_pending.is_none());
        assert_eq!(app.views[0].goto, Some((0, 0.25)));
    }

    /// A one-page PDF whose only annotation is a link running `script`, and a field `log`.
    fn linked(script: &str) -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 4 0 R] >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
            format!("<< /Type /Annot /Subtype /Link /Rect [20 20 180 60] /Border [0 0 0] /A << /S /JavaScript /JS ({script}) >> >>"),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        pdf
    }

    #[test]
    fn schematic_link_requests_flat_component_properties() {
        let mut app = PdfCraftApp::new();
        let pdf = linked("app.popUpMenu\\('Value: 10k', 'Part: R1'\\);");
        app.open_bytes("schematic.pdf", None, pdf).unwrap();
        let id = app.active_ids().unwrap().1;
        app.run_link_script(id, 0, 0);
        let ctx = egui::Context::default();
        for _ in 0..400 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.as_ref().is_some_and(|p| p.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let pending = app.script_pending.as_mut().expect("link script still active");
        assert!(pending.link_anchor.is_some());
        let (prompt, reply) = pending.prompt.take().expect("link requested a menu");
        assert!(matches!(&prompt, Prompt::Menu(items) if component_properties(items) && items.len() == 2));
        reply.send(None).unwrap();
    }

    /// Clicking a JavaScript link runs its action without blocking: its popup menu is the
    /// window's prompt, the answer resumes it, and its field change and openDoc consent apply.
    #[test]
    fn a_javascript_link_asks_its_menu_and_applies_its_result() {
        // An absolute path on this system (the document has no folder): on Windows, Acrobat's
        // `/C/…` form, which reads as `C:/…`.
        let path = if cfg!(windows) { "/C/abs/other.pdf" } else { "/abs/other.pdf" };
        let script = format!("getField\\('log'\\).value = app.popUpMenuEx\\({{cName:'A', cReturn:'a'}}\\) + event.type; app.openDoc\\('{path}'\\);");
        let mut app = PdfCraftApp::new();
        app.open_bytes("link.pdf", None, linked(&script)).unwrap();
        let id = app.active_ids().unwrap().1;
        app.run_link_script(id, 0, 0);
        let ctx = egui::Context::default();
        for _ in 0..400 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.as_ref().is_some_and(|p| p.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let (prompt, reply) = app.script_pending.as_mut().unwrap().prompt.take().unwrap();
        assert!(matches!(&prompt, Prompt::Menu(items) if items.len() == 1 && items[0].value == "a"), "{prompt:?}");
        reply.send(Some("a".into())).unwrap();
        for _ in 0..400 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.script_pending.is_none());
        let log = app.session.get(id).unwrap().form.iter().find(|f| f.name == "log").unwrap().value.clone();
        assert_eq!(log, ["aLink"]);
        assert!(app.pending_remote_link.as_ref().is_some_and(|p| p.path.ends_with("other.pdf")), "openDoc waits for consent");
    }
}
