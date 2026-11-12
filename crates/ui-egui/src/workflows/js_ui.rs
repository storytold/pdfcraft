//! Acrobat JavaScript in the shell: what scripts ask for (alerts, printing, navigation, links),
//! the JavaScript console (⌘J), Document JavaScripts, and Preferences ▸ JavaScript.

use egui::{Align, Layout};
use pdfcraft_engine::js::{JsOutput, Message, MessageIcon, Panel, Request};
use pdfcraft_engine::{DocId, Edit};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The JavaScript console: the input and the output so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsConsole {
    pub input: String,
    pub log: Vec<String>,
    /// The desktop console lives in its own native viewport, independently of modal dialogs.
    open: bool,
}

#[cfg(not(target_arch = "wasm32"))]
fn console_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pdfcraft-js-console")
}

#[cfg(not(target_arch = "wasm32"))]
fn restore_and_focus_viewport(ctx: &egui::Context, id: egui::ViewportId) {
    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
}

/// Document JavaScripts: the script being edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocJsDraft {
    pub name: String,
    pub script: String,
}

/// Most form script messages that wait for the toast; past it, more are left out and counted, and
/// the toast says how many once the rest have been shown. One run shows at most this many (the
/// script engine's own limit), so only runs in quick succession reach it.
const MAX_WAITING_MESSAGES: usize = 100;

/// The icon a script message's toast shows for `icon`.
pub(crate) fn message_icon(icon: MessageIcon) -> &'static str {
    match icon {
        MessageIcon::Error => "circle-x",
        MessageIcon::Warning => "triangle-alert",
        MessageIcon::Question => "circle-help",
        MessageIcon::Status => "info",
    }
}

/// What the user is told when a form's scripts were turned off: in a toast as it happens, then
/// on the notice bar for as long as the document is open.
pub(crate) fn scripts_off_notice() -> String {
    tl!("This form's scripts are turned off because one ran too long. Reopen the document to run them again.").to_string()
}

impl PdfCraftApp {
    /// Whether the console is visible on this platform.
    pub fn console_open(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.dialog == Some(crate::Dialog::JsConsole)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.js_console.open
        }
    }

    /// Open the console without taking the document window's modal input.
    pub fn open_console(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            self.dialog = Some(crate::Dialog::JsConsole);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.js_console.open
                && let Some(ctx) = &self.ctx
            {
                restore_and_focus_viewport(ctx, console_viewport_id());
            }
            self.js_console.open = true;
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        }
    }

    /// Close the console without changing any other dialog.
    pub(crate) fn close_console(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if self.dialog == Some(crate::Dialog::JsConsole) {
            self.dialog = None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.js_console.open = false;
        }
    }

    /// Act on what scripts produced in document `id`: alerts are shown, console output goes to
    /// the console, print and page requests are carried out, links wait for the user's permission
    /// (form submissions are reported, never sent).
    pub fn handle_js(&mut self, id: DocId, out: JsOutput) {
        let view = self.views.iter().position(|v| v.id == id);
        // A script that ran away turned the form's scripts off: calculations and buttons stop
        // working, and the error is only in the console. Say so once, and bring the notice bar
        // (which keeps saying so) back if it was closed. A message of the form's own, below,
        // takes the toast's place.
        if let Some(v) = view.and_then(|i| self.views.get_mut(i))
            && !v.scripts_off_noted
            && self.session.get(id).is_some_and(|d| d.xfa_scripts_off())
        {
            v.scripts_off_noted = true;
            v.notice_dismissed = false;
            self.notify(scripts_off_notice());
        }
        if out.is_empty() {
            return;
        }
        self.js_console.log.extend(out.console.iter().cloned());
        for e in &out.errors {
            self.js_console.log.push(format!("Error: {e}"));
        }
        for r in out.requests {
            match r {
                Request::Print => {
                    if let Some(i) = view {
                        // The print dialog reads the active document throughout its lifetime.
                        // Keep the source selected; restoring the comparison tab would retarget it.
                        self.active = Some(i);
                        self.open_print();
                    }
                }
                Request::GoToPage(p) => {
                    if let Some(i) = view {
                        self.views[i].go_to_page(p);
                    }
                }
                Request::LaunchUrl(u) => self.request_document_url(&u, crate::LinkOrigin::Script),
                Request::Submit(u) => self.notify_fmt(
                    "The form asks to be submitted to {u}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                    &[("u", &u)],
                ),
                Request::SaveAs => {
                    if let Some(i) = view {
                        self.save_view(i, crate::SaveTarget::As);
                    }
                }
                Request::OpenAttachment { name, open } => self.open_script_attachment(id, &name, open),
                Request::ShowPanel(p) => {
                    let panel = match p {
                        Panel::Bookmarks => crate::RightPanel::Bookmarks,
                        Panel::Pages => crate::RightPanel::Pages,
                        Panel::Attachments => crate::RightPanel::Attachments,
                        Panel::Signatures => crate::RightPanel::Signatures,
                        Panel::Layers => crate::RightPanel::Layers,
                    };
                    // Show/Hide: a panel already showing closes.
                    self.right = (self.right != Some(panel)).then_some(panel);
                }
                Request::Zoom(z) => {
                    if let Some(i) = view {
                        self.views[i].zoom = (z / 100.0) as f32;
                        self.views[i].fit = crate::canvas::Fit::None;
                    }
                }
                Request::Highlight(on) => self.script_highlight(id, on),
                Request::Focus(name) => self.focus_script_field(id, view, &name, None),
                Request::FocusField { name, object_id } => self.focus_script_field(id, view, &name, Some(object_id)),
                Request::Beep | Request::Reset(_) => {}
            }
        }
        self.show_script_messages(out.alerts);
    }

    fn focus_script_field(&mut self, id: DocId, view: Option<usize>, name: &str, object_id: Option<u64>) {
        if let Some(i) = view
            && let Some(f) = self.session.get(id).and_then(|d| {
                d.form.iter().find(|f| object_id.map_or_else(|| f.name == name, |key| pdfcraft_engine::js::field_object_id(f.obj) == key))
            })
            && let Some((wi, widget)) = f.widgets.iter().enumerate().find(|(_, w)| !w.hidden && w.page.is_some())
        {
            let page = widget.page.unwrap_or(0);
            if self.views[i].page_screen_rect(page).is_none() {
                self.views[i].go_to_page(page);
            }
            self.views[i].forms.focus = Some(crate::forms_ui::open_focus(f, wi));
        }
    }

    /// Show form script messages `messages` in order, each in the toast with its title and icon.
    /// The first takes the toast at once when it is free; otherwise it waits, like the rest, for
    /// what the toast shows (another message, or a notice such as the run's own submit request)
    /// to go ([`Self::next_script_message`]).
    pub fn show_script_messages(&mut self, messages: Vec<Message>) {
        let room = MAX_WAITING_MESSAGES.saturating_sub(self.script_messages.len());
        let left_out = u64::try_from(messages.len().saturating_sub(room)).unwrap_or(u64::MAX);
        self.script_messages_dropped = self.script_messages_dropped.saturating_add(left_out);
        self.script_messages.extend(messages.into_iter().take(room));
        if self.toast.is_none() {
            self.next_script_message();
        }
    }

    /// A form button's `exportDataObject`: the attached file `name` (its key in the document's
    /// embedded files, else its file name), opened in a new tab when the script asks to open a
    /// PDF, and otherwise offered to save. Other files aren't launched in other programs.
    fn open_script_attachment(&mut self, id: DocId, name: &str, open: bool) {
        let Some(doc) = self.session.get(id) else { return };
        let keyed = |a: &pdfcraft_render::Attachment| matches!(&a.source, pdfcraft_render::AttachmentSource::Document { key } if key == name);
        let index = doc.info.attachments.iter().position(keyed).or_else(|| doc.info.attachments.iter().position(|a| a.name == name));
        let Some(i) = index else {
            // The name is the form's own text: shortened, as file names are elsewhere.
            let shown = crate::dialogs::shorten_middle(name, 80);
            self.notify_fmt("The form asks for the attached file {name}, which this document doesn't have.", &[("name", &shown)]);
            return;
        };
        // Whether it is a PDF is decided by its bytes, not its name (`attachment_action`).
        self.attachment_action(id, i, open);
    }

    /// A form's script turned field highlighting on or off (`app.runtimeHighlight`): as the
    /// Highlight fields button does, for document `id` and the next ones.
    pub(crate) fn script_highlight(&mut self, id: DocId, on: bool) {
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.highlight_fields = on;
        }
        self.view_defaults.highlight_fields = on;
        self.session.set_field_highlight(on);
    }

    /// Put the next waiting script message in the toast, or, once none waits, a notice of how many
    /// the queue had no room for. False when there is neither.
    pub fn next_script_message(&mut self) -> bool {
        let Some(m) = self.script_messages.pop_front() else {
            // Every message the queue held has been shown: say how many it had no room for.
            if self.script_messages_dropped == 0 {
                return false;
            }
            let n = std::mem::take(&mut self.script_messages_dropped);
            self.notify(crate::i18n::trn(
                crate::i18n::current(),
                n,
                "{n} more message from the form's scripts was left out.",
                "{n} more messages from the form's scripts were left out.",
            ));
            // Kept with the toast until it has been shown its time (the toast's own expiry).
            self.toast_dropped = n;
            return true;
        };
        self.toast = Some((m.text.clone(), 0.0));
        self.toast_script = Some(m);
        true
    }

    /// Run document `id`'s catalog JavaScript for `event` (Will Save before a save, Will Print
    /// before printing) and show what it said. What it changed is part of what is saved or
    /// printed. A failing script is reported and the save or print still goes ahead.
    pub(crate) fn run_document_actions(&mut self, id: DocId, event: pdfcraft_engine::js::DocumentScriptEvent) {
        let before = self.session.get(id).map(|d| d.edit_generation());
        let out = self.session.run_document_actions(id, event);
        if self.session.get(id).map(|d| d.edit_generation()) != before
            && let Some(i) = self.views.iter().position(|v| v.id == id)
            && let Some(info) = self.session.get(id).map(|d| d.info.clone())
        {
            self.views[i].document_changed(&info);
        }
        if !out.is_empty() {
            self.handle_js(id, out);
        }
    }

    /// Run a push button's JavaScript (its Mouse Up action).
    pub fn run_button_script(&mut self, id: DocId, field: &str, script: &str) {
        self.run_button_script_target(id, field, None, script);
    }
    /// Run the action of the exact clicked button, preserving legacy name-only entry points.
    pub fn run_button_script_target(&mut self, id: DocId, field: &str, identity: Option<u64>, script: &str) {
        // A script reads the fields: include what's still being typed in one (#166).
        if !self.commit_form_typing() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.session.get(id).is_some_and(|d| d.info.xfa.is_none()) {
            self.start_ui_script(id, script, Some(field), identity);
            return;
        }
        match self.session.run_javascript_target(id, script, Some(field), identity) {
            Ok(o) => {
                if let Some(i) = self.views.iter().position(|v| v.id == id)
                    && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                {
                    self.views[i].document_changed(&info);
                }
                let out = JsOutput {
                    alerts: o.alerts,
                    console: o.console,
                    requests: o.requests,
                    errors: o.error.into_iter().collect(),
                    doc_errors: Vec::new(),
                };
                self.handle_js(id, out);
            }
            Err(e) => self.notify_fmt("{field}: {e}", &[("field", field), ("e", &e.to_string())]),
        }
    }

    /// Prepare a form ▸ detect fields from the page's blanks, lines and boxes.
    pub fn detect_fields(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        match self.session.auto_detect_fields(id, &[]) {
            Ok(names) if names.is_empty() => self.notify_tr("No form fields were detected"),
            Ok(names) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                if names.len() == 1 {
                    self.notify_tr("Detected 1 form field");
                } else {
                    self.notify_fmt("Detected {n} form fields", &[("n", &names.len().to_string())]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The console's Run: evaluate the input in the active document.
    pub fn run_console(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let script = self.js_console.input.clone();
        if script.trim().is_empty() {
            return;
        }
        self.js_console.log.push(format!("> {}", script.trim()));
        match self.session.run_javascript(id, &script, None) {
            Ok(o) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                let (error, result) = (o.error.clone(), o.result.clone());
                #[cfg(not(target_arch = "wasm32"))]
                let has_alerts = !o.alerts.is_empty();
                let out = JsOutput { alerts: o.alerts, console: o.console, requests: o.requests, errors: Vec::new(), doc_errors: Vec::new() };
                self.handle_js(id, out);
                // Alerts and document dialogs appear in the main viewport. Bring that window
                // forward when a script started in the separate console needs the user's answer.
                #[cfg(not(target_arch = "wasm32"))]
                if (has_alerts || self.modal_open())
                    && let Some(ctx) = &self.ctx
                {
                    restore_and_focus_viewport(ctx, egui::ViewportId::ROOT);
                }
                match error {
                    Some(e) => self.js_console.log.push(format!("Error: {e}")),
                    None => self.js_console.log.extend(result),
                }
            }
            Err(e) => self.js_console.log.push(format!("Error: {e}")),
        }
    }
}

/// Draw the desktop console in a separate operating-system window. An egui backend without
/// multiple-viewports support embeds it as a movable window in the parent instead.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn show_console(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.console_open() {
        return;
    }
    let id = console_viewport_id();
    let builder = egui::ViewportBuilder::default()
        .with_title(tl!("JavaScript Console"))
        .with_app_id("ai.storyteller.pdfcraft")
        .with_inner_size([680.0, 440.0])
        .with_min_inner_size([450.0, 300.0]);
    ctx.show_viewport_immediate(id, builder, |ui, class| {
        let close = if class == egui::ViewportClass::EmbeddedWindow {
            scrolling_console_body(ui, app)
        } else {
            let close = egui::CentralPanel::default().show(ui, |ui| scrolling_console_body(ui, app)).inner;
            close || ui.input(|i| i.viewport().close_requested())
        };
        if close {
            app.close_console();
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn scrolling_console_body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let t = Tokens::get(ui.ctx());
            console_body(ui, app, &t)
        })
        .inner
}

fn buttons(ui: &mut egui::Ui, primary: &str, primary_enabled: bool, others: &[&str]) -> Option<String> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Stable ids: the console's output above changes how many widgets come first.
            // Labels are translated for display; the returned id stays English.
            let primary_response =
                ui.push_id(primary, |ui| ui.add_enabled_ui(primary_enabled, |ui| widgets::pill_button(ui, tl!(primary), true)).inner).inner;
            if primary_response.clicked() {
                clicked = Some(primary.to_string());
            }
            for o in others {
                if ui.push_id(o, |ui| widgets::pill_button(ui, tl!(o), false)).inner.clicked() {
                    clicked = Some(o.to_string());
                }
            }
        })
    });
    clicked
}

/// The JavaScript console. Returns `true` to close.
pub(crate) fn console_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("JavaScript Console")).font(theme::semibold(18.0)));
    if let Some(name) = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|doc| doc.name.as_str()) {
        ui.label(egui::RichText::new(name).color(t.text_muted));
    } else {
        ui.label(egui::RichText::new(tl!("Open a document first")).color(t.text_muted));
    }
    // A prompt in the document window still owns its answer. Keep the separate console from
    // changing the document until that prompt is resolved.
    let can_run = app.active_ids().is_some() && (app.dialog == Some(crate::Dialog::JsConsole) || !app.modal_open());
    ui.add_space(6.0);
    if !app.session.javascript() {
        ui.label(egui::RichText::new(tl!("JavaScript is turned off (Preferences ▸ JavaScript).")).small().color(t.text_muted));
    }
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(160.0);
            if app.js_console.log.is_empty() {
                ui.label(egui::RichText::new(tl!("Output appears here.")).color(t.text_muted));
            }
            for line in &app.js_console.log {
                ui.label(egui::RichText::new(line).monospace());
            }
        });
    });
    ui.add_space(6.0);
    let input = ui.add_enabled(
        can_run,
        crate::widgets::block(&mut app.js_console.input)
            .code_editor()
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text(tl!("JavaScript, e.g. getField(\"total\").value"))
            .id_salt("js-console-input"),
    );
    let run_key = input.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.add_space(8.0);
    match buttons(ui, "Run", can_run, &["Close", "Clear"]).as_deref() {
        Some("Run") => app.run_console(),
        Some("Clear") => app.js_console.log.clear(),
        Some("Close") => return true,
        _ if run_key => app.run_console(),
        _ => {}
    }
    false
}

/// Document JavaScripts: list, edit, add and delete. Returns `true` to close.
pub(crate) fn document_js_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Document JavaScripts")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let scripts = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.document_scripts()).unwrap_or_default();
    let mut edit: Option<Edit> = None;
    ui.horizontal(|ui| {
        ui.label(tl!("Script Name:"));
        ui.add(crate::widgets::line(&mut app.doc_js.name).desired_width(240.0).id_salt("doc-js-name"));
    });
    ui.add_space(4.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(120.0).id_salt("doc-js-list").show(ui, |ui| {
            ui.set_width(ui.available_width());
            if scripts.is_empty() {
                ui.label(egui::RichText::new(tl!("This document has no document-level scripts.")).color(t.text_muted));
            }
            for (name, js) in &scripts {
                if ui.selectable_label(app.doc_js.name == *name, name).clicked() {
                    app.doc_js = DocJsDraft { name: name.clone(), script: js.clone() };
                }
            }
        });
    });
    ui.add_space(6.0);
    ui.add(crate::widgets::block(&mut app.doc_js.script).code_editor().desired_rows(8).desired_width(f32::INFINITY).id_salt("doc-js-script"));
    ui.add_space(8.0);
    let name = app.doc_js.name.trim().to_string();
    let close = match buttons(ui, "Save", true, &["Close", "Delete"]).as_deref() {
        Some("Save") if !name.is_empty() => {
            edit = Some(Edit::SetDocumentScript { name, script: Some(app.doc_js.script.clone()) });
            false
        }
        Some("Delete") if scripts.iter().any(|(n, _)| *n == name) => {
            edit = Some(Edit::SetDocumentScript { name, script: None });
            app.doc_js = DocJsDraft::default();
            false
        }
        Some("Close") => true,
        _ => false,
    };
    if let Some(e) = edit {
        app.apply_edit(e);
    }
    close
}

#[expect(unused_imports, reason = "re-exported for tests and backward compatibility")]
pub(crate) use crate::dialogs::preferences::{
    PREFERENCES_BUTTON_ROW, PREFERENCES_MIN_SCROLL, date_language_picker_name, preferences_body, preferences_buttons, preferences_header,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reopening_a_minimized_console_restores_then_focuses_it() {
        let ctx = egui::Context::default();
        let id = console_viewport_id();
        let mut app = PdfCraftApp::new();
        app.open_console();
        app.ctx = Some(ctx.clone());
        let mut input = egui::RawInput::default();
        input.viewports.insert(id, egui::ViewportInfo { minimized: Some(true), ..Default::default() });
        let output = ctx.run_logic(&input, |_| app.open_console());
        assert_eq!(output.viewport_commands[&id], [egui::ViewportCommand::Minimized(false), egui::ViewportCommand::Focus]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn render_native_console(app: &mut PdfCraftApp, child_info: egui::ViewportInfo, size: egui::Vec2) {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        ctx.set_embed_viewports(false);
        egui::Context::set_immediate_viewport_renderer(move |ctx, immediate| {
            let ids = immediate.ids;
            let mut viewport_ui = immediate.viewport_ui_cb;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    viewport_id: ids.this,
                    viewports: [(ids.this, child_info.clone())].into_iter().collect(),
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| viewport_ui(ui),
            );
            output.textures_delta.clear();
        });
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))), ..Default::default() },
            |ui| show_console(app, ui.ctx()),
        );
        output.textures_delta.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_console_close_request_closes_only_the_console() {
        let mut app = PdfCraftApp::new();
        app.dialog = Some(crate::Dialog::Preferences);
        app.open_console();
        let mut child = egui::ViewportInfo::default();
        child.events.push(egui::ViewportEvent::Close);
        render_native_console(&mut app, child, egui::vec2(680.0, 440.0));
        assert!(!app.console_open());
        assert_eq!(app.dialog, Some(crate::Dialog::Preferences));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn minimum_console_size_can_scroll_the_close_button_into_view() {
        use egui_kittest::{Harness, kittest::Queryable};

        let previous_language = crate::i18n::current();
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let app = PdfCraftApp::new();
        let mut h = Harness::builder()
            .with_size(egui::vec2(450.0, 300.0))
            .with_context_setup(|ctx| ctx.set_fonts(crate::theme::font_definitions()))
            .build_ui_state(
                |ui, app| {
                    scrolling_console_body(ui, app);
                },
                app,
            );
        h.get_by_label("Close").scroll_to_me();
        h.run();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(450.0, 300.0));
        let close = h.get_by_label("Close").rect();
        assert!(viewport.contains_rect(close), "Close at {close:?} must fit within {viewport:?}");
        crate::i18n::set_current(previous_language);
    }

    #[test]
    fn date_language_names_fall_back_when_the_native_name_cannot_be_drawn() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        let date_language = |code: &str| pdfcraft_engine::dates::DATE_LANGUAGES.iter().find(|l| l.code == code).expect("date language");
        let hindi = date_language("hi");
        assert_eq!(date_language_picker_name(hindi, |_| true), hindi.name, "the native name is kept when it can be drawn");
        assert_eq!(date_language_picker_name(hindi, |_| false), "Hindi", "no Devanagari face: show the English name");
        let chinese = date_language("zh-hans");
        assert_eq!(date_language_picker_name(chinese, |_| false), "Simplified Chinese");
        for language in &pdfcraft_engine::dates::DATE_LANGUAGES {
            assert!(!date_language_picker_name(language, |_| false).is_empty(), "{}", language.code);
        }
    }
}
