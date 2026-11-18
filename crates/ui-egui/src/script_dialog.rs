//! Script prompts suspend the document worker, never the egui event loop.
use crate::PdfCraftApp;
use pdfcraft_engine::{
    DocId,
    js::{InteractiveEvent, JsOutput, MenuItem, PendingScript, Prompt},
};
use std::sync::mpsc::{Sender, TryRecvError};
use std::time::{Duration, Instant};

fn dialog_values(elements: &[pdfcraft_engine::js::DialogElement], values: &mut std::collections::BTreeMap<String, serde_json::Value>) {
    for e in elements {
        if !e.id.is_empty() {
            values.insert(e.id.clone(), e.value.clone());
        }
        dialog_values(&e.children, values);
    }
}
fn dialog_changes(
    elements: &[pdfcraft_engine::js::DialogElement],
    values: &std::collections::BTreeMap<String, serde_json::Value>,
    changed: &mut serde_json::Map<String, serde_json::Value>,
) {
    for e in elements.iter().filter(|e| e.enabled && e.visible) {
        if let Some(value) = values.get(&e.id).filter(|v| **v != e.value) {
            changed.insert(e.id.clone(), value.clone());
        }
        dialog_changes(&e.children, values, changed);
    }
}
fn select_list(value: &mut serde_json::Value, selected: &[String], toggle: bool, path: &mut Vec<String>) {
    if let Some(entries) = value.as_object_mut() {
        for (name, value) in entries {
            path.push(name.clone());
            if let Some(rank) = value.as_f64() {
                let chosen = path == selected;
                let on = if toggle { (rank > 0.0) != chosen } else { chosen };
                *value = serde_json::json!(if on { rank.abs().max(1.0) } else { -rank.abs().max(1.0) });
            } else {
                select_list(value, selected, toggle, path);
            }
            path.pop();
        }
    }
}
fn list_rows(ui: &mut egui::Ui, value: &serde_json::Value, path: &mut Vec<String>, chosen: &mut Option<Vec<String>>) {
    let Some(entries) = value.as_object() else { return };
    let mut entries: Vec<_> = entries.iter().collect();
    entries.sort_by(|(a, av), (b, bv)| av.as_f64().unwrap_or(0.0).abs().total_cmp(&bv.as_f64().unwrap_or(0.0).abs()).then(a.cmp(b)));
    for (label, value) in entries {
        path.push(label.clone());
        if let Some(rank) = value.as_f64() {
            if ui.selectable_label(rank > 0.0, label).clicked() {
                *chosen = Some(path.clone());
            }
        } else {
            egui::CollapsingHeader::new(label).default_open(true).show(ui, |ui| list_rows(ui, value, path, chosen));
        }
        path.pop();
    }
}
fn radio_peers(elements: &[pdfcraft_engine::js::DialogElement], group: &str, values: &mut std::collections::BTreeMap<String, serde_json::Value>) {
    for e in elements {
        if e.kind == "radio" && e.group == group {
            values.insert(e.id.clone(), false.into());
        }
        radio_peers(&e.children, group, values);
    }
}
fn custom_elements(
    ui: &mut egui::Ui,
    elements: &[pdfcraft_engine::js::DialogElement],
    all: &[pdfcraft_engine::js::DialogElement],
    values: &mut std::collections::BTreeMap<String, serde_json::Value>,
    focus: &str,
    needs_focus: &mut bool,
    event: &mut Option<String>,
) {
    for e in elements.iter().filter(|e| e.visible) {
        ui.push_id((&e.id, &e.name, &e.kind), |ui| {
            ui.add_enabled_ui(e.enabled, |ui| {
                let value = values.get(&e.id).cloned().unwrap_or_default();
                match e.kind.as_str() {
                    "image" => {
                        if let Some(image) = dialog_image(&value) {
                            let id = ui.scope_id().with(value.to_string());
                            let texture = ui.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
                                let texture = ui.ctx().load_texture("dialog image", image, egui::TextureOptions::LINEAR);
                                ui.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
                                texture
                            });
                            ui.image((texture.id(), texture.size_vec2()));
                        }
                    }
                    "view" | "cluster" | "radio_group" | "tab_group" | "tab" => {
                        if e.kind == "cluster" && !e.name.is_empty() {
                            ui.strong(&e.name);
                        }
                        if e.alignment == "align_row" {
                            ui.horizontal_wrapped(|ui| custom_elements(ui, &e.children, all, values, focus, needs_focus, event));
                        } else {
                            custom_elements(ui, &e.children, all, values, focus, needs_focus, event);
                        }
                    }
                    "link_text" => {
                        if ui.link(value.as_str().unwrap_or(&e.name)).clicked() {
                            *event = Some(e.id.clone());
                        }
                    }
                    "static_text" => {
                        let label = value.as_str().unwrap_or(&e.name);
                        if e.red {
                            ui.colored_label(egui::Color32::RED, label);
                        } else {
                            ui.label(label);
                        }
                    }
                    "edit_text" => {
                        let text = values.entry(e.id.clone()).or_insert_with(|| "".into());
                        if !text.is_string() {
                            *text = if value.is_number() || value.is_boolean() { value.to_string().into() } else { "".into() };
                        }
                        if let Some(text) = text.as_str().map(str::to_owned) {
                            let mut text = text;
                            let edit = if e.multiline {
                                egui::TextEdit::multiline(&mut text).desired_rows(4)
                            } else {
                                egui::TextEdit::singleline(&mut text)
                            };
                            let response = ui.add(edit.password(e.password).interactive(!e.readonly).char_limit(8 << 20));
                            if *needs_focus && (focus == e.id || focus.is_empty()) {
                                response.request_focus();
                                *needs_focus = false;
                            }
                            if response.changed() {
                                values.insert(e.id.clone(), text.into());
                            }
                            // Moving from an unchanged field into a popup must not resume the
                            // script: its next dialog snapshot would close the open dropdown.
                            if response.lost_focus() && values.get(&e.id).is_some_and(|value| *value != e.value) {
                                *event = Some(e.id.clone());
                            }
                        }
                    }
                    "check_box" | "radio" => {
                        let mut checked = value.as_bool().unwrap_or(false);
                        let changed = if e.kind == "radio" {
                            ui.add(crate::widgets::radio(checked, &e.name)).clicked()
                        } else {
                            ui.add(crate::widgets::checkbox(&mut checked, &e.name)).changed()
                        };
                        if changed {
                            if e.kind == "radio" {
                                radio_peers(all, &e.group, values);
                                checked = true;
                            }
                            values.insert(e.id.clone(), checked.into());
                            *event = Some(e.id.clone());
                        }
                    }
                    "popup" | "list_box" | "hier_list_box" => {
                        let mut chosen = None;
                        if e.kind == "popup" {
                            let label = value
                                .as_object()
                                .and_then(|o| o.iter().find(|(_, v)| v.as_f64().is_some_and(|n| n > 0.0)))
                                .map_or("", |(k, _)| k.as_str());
                            let dialog_layer = ui.layer_id();
                            craft_ui::ComboBox::from_id_salt("popup").selected_text(label).show_ui(ui, |ui| {
                                // Keep the list above its modal even when the modal is raised
                                // to retain input priority over other document dialogs.
                                ui.ctx().set_sublayer(dialog_layer, ui.layer_id());
                                list_rows(ui, &value, &mut vec![], &mut chosen);
                            });
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("list")
                                .max_height(180.0)
                                .show(ui, |ui| list_rows(ui, &value, &mut vec![], &mut chosen));
                        }
                        if let Some(chosen) = chosen {
                            let mut value = value;
                            let toggle = e.multiselect && ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                            select_list(&mut value, &chosen, toggle, &mut vec![]);
                            values.insert(e.id.clone(), value);
                            *event = Some(e.id.clone());
                        }
                    }
                    "button" => {
                        if ui.button(&e.name).clicked() {
                            *event = Some(e.id.clone());
                        }
                    }
                    "ok" | "ok_cancel" | "ok_cancel_other" => {
                        ui.horizontal(|ui| {
                            if ui.button(tl!("OK")).clicked() {
                                *event = Some("ok".into());
                            }
                            if e.kind != "ok" && ui.button(tl!("Cancel")).clicked() {
                                *event = Some("cancel".into());
                            }
                            if e.kind == "ok_cancel_other" && ui.button(&e.other_name).clicked() {
                                *event = Some("other".into());
                            }
                        });
                    }
                    "gap" => {
                        ui.add_space(8.0);
                    }
                    _ => {
                        if !e.children.is_empty() {
                            custom_elements(ui, &e.children, all, values, focus, needs_focus, event);
                        }
                    }
                }
            })
        });
    }
}

fn dialog_image(value: &serde_json::Value) -> Option<egui::ColorImage> {
    let width = usize::try_from(value.get("width")?.as_u64()?).ok()?;
    let height = usize::try_from(value.get("height")?.as_u64()?).ok()?;
    let size = width.checked_mul(height)?.checked_mul(4)?;
    if width == 0 || height == 0 || size > 1 << 20 {
        return None;
    }
    let text = value.get("argb")?.as_str()?;
    if text.len() != size.checked_mul(2)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(size);
    for pixel in text.as_bytes().as_chunks::<8>().0 {
        let mut argb = [0u8; 4];
        for (channel, pair) in argb.iter_mut().zip(pixel.as_chunks::<2>().0) {
            *channel = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        rgba.extend_from_slice(&[argb[1], argb[2], argb[3], argb[0]]);
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba))
}

pub(crate) struct Pending {
    #[cfg(not(target_arch = "wasm32"))]
    token: std::sync::Arc<()>,
    run: PendingScript,
    prompt: Option<(Prompt, Sender<Option<String>>)>,
    text: String,
    focus: bool,
    started: Instant,
    zoom: f32,
    page: usize,
    serial: u32,
    dialog_values: std::collections::BTreeMap<String, serde_json::Value>,
    progress: Option<pdfcraft_engine::js::Progress>,
    file_read: Option<(u32, std::sync::mpsc::Receiver<Result<String, String>>)>,
}
impl Pending {
    #[cfg(not(target_arch = "wasm32"))]
    fn accepts_file(&self, id: DocId, serial: u32, token: &std::sync::Arc<()>) -> bool {
        std::sync::Arc::ptr_eq(&self.token, token)
            && self.run.id == id
            && self.serial == serial
            && matches!(self.prompt, Some((Prompt::File, _)))
            && self.file_read.is_none()
    }
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
    pub(crate) fn show_document_tool_buttons(&mut self, ui: &mut egui::Ui) {
        let Some((_, id)) = self.active_ids() else { return };
        let buttons = self.session.get(id).map(|d| d.script_tool_buttons().to_vec()).unwrap_or_default();
        if buttons.is_empty() {
            return;
        }
        let enabled = !self.modal_open() && self.script_pending.is_none();
        let mut clicked = None;
        ui.horizontal_wrapped(|ui| {
            for button in buttons {
                if ui.add_enabled(enabled, egui::Button::new(&button.label)).on_hover_text(&button.tooltip).clicked() {
                    clicked = Some(button.name);
                }
            }
        });
        if let Some(name) = clicked {
            match self.session.script_tool_button_source(id, &name) {
                Ok(source) => self.start_ui_script(id, &source, None, None),
                Err(e) => self.notify(e.to_string()),
            }
        }
    }
    /// Start one explicit button interaction with captured document/view state and leave egui responsive.
    pub(crate) fn start_ui_script(&mut self, id: DocId, script: &str, field: Option<&str>, identity: Option<u64>) {
        self.start_script_with(id, |s| s.start_javascript_target(id, script, field, identity));
    }
    /// Start link `index` on page `page`'s JavaScript action (0-based) as a button's runs: its
    /// menus and questions are asked without blocking the window (#2081).
    pub(crate) fn start_link_script(&mut self, id: DocId, page: usize, index: usize) {
        self.start_script_with(id, |s| s.start_link_javascript(id, page, index));
    }
    fn start_script_with(
        &mut self,
        id: DocId,
        start: impl FnOnce(&mut pdfcraft_engine::Session) -> Result<PendingScript, pdfcraft_engine::EditError>,
    ) {
        if self.script_pending.is_some() {
            self.notify("Another form script is active; this click was not run");
            return;
        }
        let Some(view) = self.views.iter().find(|v| v.id == id) else { return };
        let (zoom, page, highlight) = (view.zoom, view.current, view.highlight_fields);
        self.session.set_javascript_view(id, zoom as f64 * 100.0, highlight, page);
        match start(&mut self.session) {
            Ok(run) => {
                self.script_pending = Some(Pending {
                    #[cfg(not(target_arch = "wasm32"))]
                    token: std::sync::Arc::new(()),
                    run,
                    prompt: None,
                    text: String::new(),
                    focus: false,
                    started: Instant::now(),
                    zoom,
                    page,
                    serial: 0,
                    dialog_values: Default::default(),
                    progress: None,
                    file_read: None,
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
        if let Some((serial, read)) = pending.file_read.as_ref() {
            let serial = *serial;
            let result = match read.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("The selected-file reader stopped".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                pending.file_read = None;
                if serial == pending.serial && matches!(pending.prompt, Some((Prompt::File, _))) {
                    match result {
                        Ok(text) => reply_to_prompt(ctx, &mut pending, Some(text), false),
                        Err(error) => {
                            self.notify(error);
                            reply_to_prompt(ctx, &mut pending, None, true);
                        }
                    }
                }
            }
        }
        if (pending.prompt.is_none() && pending.started.elapsed() > pending.run.timeout)
            || (pending.prompt.is_some() && pending.started.elapsed() > Duration::from_secs(300))
        {
            self.session.timeout_javascript(pending.run);
            self.notify("The form script timed out; its result was discarded");
            return;
        }
        if pending.prompt.is_none() {
            match pending.run.receiver.try_recv() {
                Ok(InteractiveEvent::Progress(progress)) => {
                    pending.progress = Some(progress);
                    ctx.request_repaint();
                }
                Ok(InteractiveEvent::Prompt(prompt, reply)) => {
                    pending.dialog_values.clear();
                    if let Prompt::Dialog(dialog) = &prompt {
                        dialog_values(&dialog.elements, &mut pending.dialog_values);
                    }
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
        ctx.request_repaint_after(if pending.has_prompt() && pending.file_read.is_none() {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(16)
        });
        self.script_pending = Some(pending);
    }
    /// Display the current typed prompt and send one answer/cancellation without replaying script side effects.
    pub(crate) fn show_script_dialog(&mut self, ctx: &egui::Context) {
        self.show_script_dialog_with_menu(ctx, |_, _, _| None);
    }

    /// Let an alternative renderer own a menu until it returns a selection or cancellation.
    /// Returning `None` uses the built-in modal; the renderer never receives the reply sender.
    pub fn show_script_dialog_with_menu(
        &mut self,
        ctx: &egui::Context,
        render_menu: impl FnOnce(&egui::Context, DocId, &[MenuItem]) -> Option<std::task::Poll<Option<String>>>,
    ) {
        let Some(pending) = self.script_pending.as_mut() else { return };
        if let Some(progress) = pending.progress.as_ref().filter(|p| p.active) {
            egui::Window::new(tl!("JavaScript")).collapsible(false).show(ctx, |ui| {
                ui.label(&progress.text);
                ui.add(egui::ProgressBar::new(if progress.duration > 0.0 {
                    (progress.value / progress.duration).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                }));
                if ui.button(tl!("Cancel")).clicked() {
                    progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
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
        let mut choose_file = false;
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
                Prompt::File => {
                    ui.label("Select a UTF-8 text file to share with this document's script.");
                    choose_file = ui.add_enabled(pending.file_read.is_none(), egui::Button::new(tl!("Open"))).clicked();
                }
                Prompt::Dialog(dialog) => {
                    ui.label(&dialog.title);
                    let mut event = None;
                    egui::ScrollArea::vertical().max_height(600.0).show(ui, |ui| {
                        custom_elements(
                            ui,
                            &dialog.elements,
                            &dialog.elements,
                            &mut pending.dialog_values,
                            &dialog.focus,
                            &mut pending.focus,
                            &mut event,
                        );
                    });
                    if let Some(event) = event {
                        let mut changed = serde_json::Map::new();
                        dialog_changes(&dialog.elements, &pending.dialog_values, &mut changed);
                        answer = Some(serde_json::json!({"event":event,"values":changed}).to_string());
                    }
                }
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
        #[cfg(not(target_arch = "wasm32"))]
        if choose_file {
            let id = pending.run.id;
            let serial = pending.serial;
            let token = pending.token.clone();
            let ctx = ctx.clone();
            self.ask_one(crate::platform::pickers::Ask::File(rfd::AsyncFileDialog::new()), Some(id), move |app, path| {
                let Some(pending) = app.script_pending.as_mut().filter(|p| p.accepts_file(id, serial, &token)) else {
                    return;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                let wake = ctx.clone();
                let worker = std::thread::Builder::new().name("pdfcraft-script-file".into()).spawn(move || {
                    use std::io::Read;
                    let result = (|| {
                        if !std::fs::metadata(&path)?.is_file() {
                            return Err(std::io::Error::other("not a regular file"));
                        }
                        let mut text = String::new();
                        std::fs::File::open(path)?.take((8 << 20) + 1).read_to_string(&mut text)?;
                        if text.len() > 8 << 20 {
                            return Err(std::io::Error::other("limit: 8 MiB"));
                        }
                        Ok(text)
                    })()
                    .map_err(|e| format!("Couldn't read the selected UTF-8 text file: {e}"));
                    let _ = tx.send(result);
                    wake.request_repaint();
                });
                if worker.is_ok() {
                    pending.file_read = Some((serial, rx));
                } else {
                    reply_to_prompt(&ctx, pending, None, true);
                    app.notify("Couldn't start the selected-file reader");
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = choose_file;
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn file_prompt_identity_and_completion_reject_stale_results() {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        let ctx = egui::Context::default();
        let mut old = None;
        for _ in 0..2 {
            app.start_ui_script(id, "util.readFileIntoStream();", None, None);
            for _ in 0..300 {
                app.poll_script_dialog(&ctx);
                if app.script_pending.as_ref().is_some_and(Pending::has_prompt) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let p = app.script_pending.as_mut().unwrap();
            assert!(p.accepts_file(id, p.serial, &p.token));
            if let Some((serial, token)) = old.take() {
                assert_eq!(serial, p.serial);
                assert!(!p.accepts_file(id, serial, &token), "old picker cannot answer a new invocation");
                let (tx, rx) = std::sync::mpsc::channel();
                p.file_read = Some((serial.saturating_sub(1), rx));
                // An unfinished read returns to the frame; no blocking receive.
                app.poll_script_dialog(&ctx);
                assert!(app.script_pending.as_ref().unwrap().has_prompt());
                tx.send(Ok("stale".into())).unwrap();
                app.poll_script_dialog(&ctx);
                assert!(app.script_pending.as_ref().unwrap().has_prompt(), "stale completion is discarded");
                assert!(app.script_pending.as_ref().unwrap().file_read.is_none());
            } else {
                old = Some((p.serial, p.token.clone()));
                let p = app.script_pending.take().unwrap();
                app.session.discard_javascript(&p.run);
            }
        }
    }

    #[test]
    fn opening_a_dialog_dropdown_only_commits_changed_text() {
        for edited in [false, true] {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes("synthetic.pdf", None, bytes.to_vec()).unwrap();
            let id = app.active_ids().unwrap().1;
            app.start_ui_script(
                id,
                r#"app.execDialog({
                    initialize: function(d) { d.load({text: '1', pick: {Fighter: 1, Wizard: -2}}); d.focus('text'); },
                    description: {name: 'Choose class', elements: [
                        {type: 'edit_text', item_id: 'text'},
                        {type: 'popup', item_id: 'pick'}
                    ]}
                });"#,
                None,
                None,
            );
            let ctx = egui::Context::default();
            for _ in 0..300 {
                app.poll_script_dialog(&ctx);
                if app.script_pending.as_ref().is_some_and(|p| p.has_prompt()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.script_pending.as_ref().unwrap().has_prompt());
            let mut h = Harness::builder()
                .with_size(egui::vec2(800.0, 600.0))
                .with_accessibility_check(false)
                .build_ui_state(|ui, app: &mut PdfCraftApp| app.show_script_dialog(ui.ctx()), app);
            h.run();
            if edited {
                h.get_by_role(egui::accesskit::Role::TextInput).type_text("2");
                h.run();
            }
            // Harness clicks send actual pointer events, so the edit loses focus just as
            // it does when a person opens the popup (an AccessKit action would not).
            h.get_by_role(egui::accesskit::Role::ComboBox).click();
            h.run();
            assert_eq!(h.state().script_pending.as_ref().unwrap().has_prompt(), !edited);
            if !edited {
                h.get_by_label("Wizard").click();
                h.run();
                assert!(!h.state().script_pending.as_ref().unwrap().has_prompt(), "a selection still resumes the script");
            }
        }
    }

    #[test]
    fn dialog_image_reads_argb_and_bounds_dimensions() {
        let image = dialog_image(&serde_json::json!({"width":1,"height":1,"argb":"80102030"})).unwrap();
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels[0], egui::Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 0x80));
        for value in [serde_json::json!({"width":u64::MAX,"height":u64::MAX,"argb":""}), serde_json::json!({"width":1,"height":1,"argb":"invalid!"})]
        {
            assert!(dialog_image(&value).is_none());
        }
    }
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
