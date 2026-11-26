//! Acrobat JavaScript in the shell: what scripts ask for (alerts, printing, navigation, links),
//! and the JavaScript console's state.

use craft::theme::variant::Tone;
use craft::ui::notices::{NoticeEnd, PostedNotice};
use craft::ui::runtime::{frame, toast::ToastId};
use pdfcraft_engine::DocId;
use pdfcraft_engine::js::{JsOutput, Message, MessageIcon, Panel, Request};

use crate::PdfCraftApp;

/// The JavaScript console: the input and the output so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsConsole {
    pub input: String,
    pub log: Vec<String>,
}

/// Most form script messages that wait for a notice; past it, more are left out and counted, and
/// a notice says how many once the rest have been shown. One run shows at most this many (the
/// script engine's own limit), so only runs in quick succession reach it.
const MAX_WAITING_MESSAGES: usize = 100;

/// The script notice on screen, so its end is known for the app's own: one message is shown at a time, each
/// when the one before it has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptShown {
    /// A script message.
    Message(ToastId),
    /// The count of messages the queue had no room for, with that count: said again if the notice was dropped
    /// before it was shown its time.
    Count(ToastId, u64),
}

impl ScriptShown {
    fn id(self) -> ToastId {
        match self {
            ScriptShown::Message(id) | ScriptShown::Count(id, _) => id,
        }
    }
}

/// The tone a script message's notice reads in for `icon`.
pub(crate) fn message_tone(icon: MessageIcon) -> Tone {
    match icon {
        MessageIcon::Error => Tone::Bad,
        MessageIcon::Warning => Tone::Warn,
        MessageIcon::Question | MessageIcon::Status => Tone::Info,
    }
}

/// What the user is told when a form's scripts were turned off: in a toast as it happens, then
/// on the notice bar for as long as the document is open.
pub(crate) fn scripts_off_notice() -> String {
    tl!("This form's scripts are turned off because one ran too long. Reopen the document to run them again.").to_string()
}

impl PdfCraftApp {
    /// Act on what scripts produced in document `id`: alerts are shown, console output goes to
    /// the console, print and page requests are carried out, links wait for the user's permission
    /// (form submissions are reported, never sent).
    pub fn handle_js(&mut self, ctx: &egui::Context, id: DocId, out: JsOutput) {
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
                Request::OpenDoc(path) => self.request_script_document(id, &path),
                Request::LaunchUrl(u) => self.request_document_url(&u, crate::LinkOrigin::Script),
                Request::Submit(u) => self.notify_fmt(
                    "The form asks to be submitted to {u}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                    &[("u", &u)],
                ),
                Request::SaveAs => {
                    if let Some(i) = view {
                        self.save_view(ctx, i, crate::SaveTarget::As);
                    }
                }
                Request::OpenAttachment { name, open } => self.open_script_attachment(ctx, id, &name, open),
                Request::ShowPanel(p) => {
                    let panel = match p {
                        Panel::Bookmarks => crate::RightPanel::Bookmarks,
                        Panel::Pages => crate::RightPanel::Pages,
                        Panel::Attachments => crate::RightPanel::Attachments,
                        Panel::Signatures => crate::RightPanel::Signatures,
                        Panel::Layers => crate::RightPanel::Layers,
                    };
                    // Show/Hide: a panel already showing closes.
                    self.set_right((self.right() != Some(panel)).then_some(panel));
                }
                Request::Zoom(z) => {
                    if let Some(i) = view {
                        self.views[i].view.zoom = z / 100.0;
                        self.views[i].view.fit = craft::pages::Fit::None;
                    }
                }
                Request::Highlight(on) => self.script_highlight(id, on),
                Request::Focus(name) => self.focus_script_field(id, view, &name, None),
                Request::FocusField { name, object_id } => self.focus_script_field(id, view, &name, Some(object_id)),
                // The engine applies these with the script's other changes (resets, templates).
                Request::Beep | Request::Reset(_) | Request::SpawnTemplate { .. } | Request::SetTemplateHidden { .. } => {}
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
            self.views[i].session.forms.focus = Some(crate::forms_ui::open_focus(f, wi));
        }
    }

    /// Show form script messages `messages` in order, each as a notice with its title and tone.
    /// The first is posted on the next pass ([`Self::script_tick`]) when no script notice is on
    /// screen; otherwise it waits, like the rest, for the one shown to end ([`Self::script_notice_ended`]).
    pub fn show_script_messages(&mut self, messages: Vec<Message>) {
        let room = MAX_WAITING_MESSAGES.saturating_sub(self.script_messages.len());
        let left_out = u64::try_from(messages.len().saturating_sub(room)).unwrap_or(u64::MAX);
        self.script_messages_dropped = self.script_messages_dropped.saturating_add(left_out);
        self.script_messages.extend(messages.into_iter().take(room));
    }

    /// A form button's `exportDataObject`: the attached file `name` (its key in the document's
    /// embedded files, else its file name), opened in a new tab when the script asks to open a
    /// PDF, and otherwise offered to save. Other files aren't launched in other programs.
    fn open_script_attachment(&mut self, ctx: &egui::Context, id: DocId, name: &str, open: bool) {
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
        self.attachment_action(ctx, id, i, open);
    }

    /// A form's script turned field highlighting on or off (`app.runtimeHighlight`): as the
    /// Highlight fields button does, for document `id` and the next ones.
    pub(crate) fn script_highlight(&mut self, id: DocId, on: bool) {
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.session.highlight_fields = on;
        }
        self.highlight_fields = on;
        self.session.set_field_highlight(on);
    }

    /// Post the next waiting script message, or, once none waits, a notice of how many the queue had
    /// no room for. False when there is neither.
    pub fn next_script_message(&mut self, ctx: &egui::Context) -> bool {
        let Some(m) = self.script_messages.pop_front() else {
            // Every message the queue held has been shown: say how many it had no room for.
            if self.script_messages_dropped == 0 {
                return false;
            }
            let n = std::mem::take(&mut self.script_messages_dropped);
            let text = crate::i18n::trn(
                crate::i18n::current(),
                n,
                "{n} more message from the form's scripts was left out.",
                "{n} more messages from the form's scripts were left out.",
            );
            let Some(id) = self.notices.post(ctx, &PostedNotice::new(text.to_string()), &[]) else { return false };
            self.script_shown = Some(ScriptShown::Count(id, n));
            return true;
        };
        let notice = match m.title.as_ref().filter(|title| !title.is_empty()) {
            Some(title) => PostedNotice::new(title.clone()).message(m.text.clone()),
            None => PostedNotice::new(m.text.clone()),
        }
        .tone(message_tone(m.icon));
        match self.notices.post(ctx, &notice, &[]) {
            Some(id) => {
                self.script_shown = Some(ScriptShown::Message(id));
                true
            }
            // Nothing silences a script message, so this does not happen; the next one takes its place.
            None => self.next_script_message(ctx),
        }
    }

    /// A notice ended: when it is the script notice on screen, the next message is posted. A count notice
    /// the card limit dropped before its time is said again.
    pub(crate) fn script_notice_ended(&mut self, ctx: &egui::Context, end: &NoticeEnd) {
        let (NoticeEnd::Closed(id) | NoticeEnd::Expired(id) | NoticeEnd::Dropped(id) | NoticeEnd::Silenced { id, .. }) = end else { return };
        let Some(shown) = self.script_shown.filter(|shown| shown.id() == *id) else { return };
        self.script_shown = None;
        if let ScriptShown::Count(_, n) = shown
            && matches!(end, NoticeEnd::Dropped(_))
        {
            self.script_messages_dropped = self.script_messages_dropped.saturating_add(n);
        }
        self.next_script_message(ctx);
    }

    /// Each frame: the live scripts of the document being read (#169). Its page's open actions
    /// run when the page changes (after the close actions of the page it was at), and its timers
    /// when they fall due, such as an animation's frames (LaTeX `animate`). Only the active tab
    /// runs: an animation in another tab waits until its tab shows again.
    pub(crate) fn script_tick(&mut self, ctx: &egui::Context) {
        // A message queued before the window had a context is posted on the first pass.
        if self.script_shown.is_none() && !self.script_messages.is_empty() {
            // Requests from the same script may already have queued notices; post those first.
            crate::notices::post_queued(self, ctx);
            self.next_script_message(ctx);
        }
        // Nothing acts on the document under a dialog or prompt (`modal_open`): page actions and
        // timers wait, and an interval that fell behind runs once when it closes.
        if self.modal_open() {
            return;
        }
        let Some((i, id)) = self.active_ids() else { return };
        let now_ms = ctx.input(|input| input.time) * 1000.0;
        let page = self.views[i].current();
        // The page's actions, then its timers, if what the actions asked for left the viewer as it
        // was: a page they went to runs its close and open actions (next frame) before any timer,
        // and a dialog they opened holds the timers too.
        for timers in [false, true] {
            if timers && (self.modal_open() || self.active_ids() != Some((i, id)) || self.views[i].current() != page) {
                frame::request_repaint_after(ctx, 0.0);
                break;
            }
            // The field being edited as it was, so a script changing it shows in its editor while
            // a draft the user already typed into is kept (as `run_field_triggers` does).
            let before: Option<(String, Vec<String>)> = self.views[i].session.forms.focus.as_ref().and_then(|e| {
                crate::forms_ui::focused_field(e, &self.session.get(id)?.form).map(|f| (crate::forms_ui::draft_text(f), f.value.clone()))
            });
            let run = if timers { self.session.run_timers(id, now_ms) } else { self.session.show_page(id, page, now_ms) };
            if run.is_empty() {
                continue;
            }
            if run.edited {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(ctx, &info);
                }
                if let Some(before) = &before
                    && let Some(draft) = self.views[i].session.forms.focus.as_mut()
                    && let Some(f) = self.session.get(id).and_then(|d| crate::forms_ui::focused_field(draft, &d.form))
                {
                    crate::forms_ui::refresh_draft(draft, (&before.0, &before.1), (crate::forms_ui::draft_text(f), &f.value));
                }
            } else if run.layers {
                self.views[i].invalidate_content();
            } else {
                for p in &run.pages {
                    self.views[i].page_changed(*p);
                }
            }
            self.handle_js(ctx, id, run.output);
            frame::request_repaint_after(ctx, 0.0);
        }
        if let Some(at) = self.session.next_timer(id) {
            let wait = ((at - now_ms) / 1000.0).clamp(0.0, 3600.0);
            frame::request_repaint_after(ctx, wait);
        }
    }

    /// Run document `id`'s catalog JavaScript for `event` (Will Save before a save, Will Print
    /// before printing) and show what it said. What it changed is part of what is saved or
    /// printed. A failing script is reported and the save or print still goes ahead.
    pub(crate) fn run_document_actions(&mut self, ctx: &egui::Context, id: DocId, event: pdfcraft_engine::js::DocumentScriptEvent) {
        let before = self.session.get(id).map(|d| d.edit_generation());
        let out = self.session.run_document_actions(id, event);
        if self.session.get(id).map(|d| d.edit_generation()) != before
            && let Some(i) = self.views.iter().position(|v| v.id == id)
            && let Some(info) = self.session.get(id).map(|d| d.info.clone())
        {
            self.views[i].document_changed(ctx, &info);
        }
        if !out.is_empty() {
            self.handle_js(ctx, id, out);
        }
    }

    /// Run a push button's JavaScript (its Mouse Up action).
    pub fn run_button_script(&mut self, ctx: &egui::Context, id: DocId, field: &str, script: &str) {
        self.run_button_script_target(ctx, id, field, None, script);
    }
    /// Run the action of the exact clicked button, preserving legacy name-only entry points.
    pub fn run_button_script_target(&mut self, ctx: &egui::Context, id: DocId, field: &str, identity: Option<u64>, script: &str) {
        // A script reads the fields: include what's still being typed in one (#166).
        if !self.commit_form_typing(ctx) {
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
                    self.views[i].document_changed(ctx, &info);
                }
                let out = JsOutput {
                    alerts: o.alerts,
                    console: o.console,
                    requests: o.requests,
                    errors: o.error.into_iter().collect(),
                    doc_errors: Vec::new(),
                };
                self.handle_js(ctx, id, out);
            }
            Err(e) => self.notify_fmt("{field}: {e}", &[("field", field), ("e", &e.to_string())]),
        }
    }

    /// `app.openDoc(path)`: resolve the path against document `id`'s folder (no file is touched)
    /// and ask before opening it, as for a link to another PDF given by an absolute path; a script
    /// never opens a file unasked. A request while another link waits for an answer is dropped,
    /// as a link's is (#2081).
    #[cfg(not(target_arch = "wasm32"))]
    fn request_script_document(&mut self, id: DocId, path: &str) {
        if self.pending_remote_link.is_some() || self.pending_link.is_some() {
            return;
        }
        let source = self.session.get(id).and_then(|d| d.path.clone());
        match pdfcraft_engine::js::resolve_javascript_document_path(source.as_deref().map(std::path::Path::new), path) {
            Ok(path) => {
                self.pending_remote_link =
                    Some(crate::PendingRemoteLink { source: id, path, destination: pdfcraft_render::RemoteDestination::Default })
            }
            Err(e) => self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &e.to_string())]),
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn request_script_document(&mut self, _id: DocId, _path: &str) {
        self.notify_tr("Local cross-PDF links require the desktop app");
    }

    /// Run the JavaScript action of link `index` on page `page` (0-based; its place in the page's
    /// annotations), clicked in document `id` (#2081). A failed script changes nothing.
    pub fn run_link_script(&mut self, ctx: &egui::Context, id: DocId, page: usize, index: usize) {
        if !self.commit_form_typing(ctx) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.start_link_script(id, page, index);
        }
        // Script getters read the clicked view's zoom, page and field highlight, as on the
        // desktop, where starting the script sets them.
        #[cfg(target_arch = "wasm32")]
        if let Some(view) = self.views.iter().find(|v| v.id == id) {
            let (zoom, current, highlight) = (view.zoom(), view.current(), view.session.highlight_fields);
            self.session.set_javascript_view(id, zoom * 100.0, highlight, current);
        }
        #[cfg(target_arch = "wasm32")]
        match self.session.run_link_javascript(id, page, index, &[]) {
            Ok(o) => {
                if let Some(i) = self.views.iter().position(|v| v.id == id)
                    && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                {
                    self.views[i].document_changed(ctx, &info);
                }
                let out = JsOutput {
                    alerts: o.alerts,
                    console: o.console,
                    requests: o.requests,
                    errors: o.error.into_iter().collect(),
                    doc_errors: Vec::new(),
                };
                self.handle_js(ctx, id, out);
            }
            Err(e) => self.notify(e.to_string()),
        }
    }

    /// Prepare a form ▸ detect fields from the page's blanks, lines and boxes.
    pub fn detect_fields(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        match self.session.auto_detect_fields(id, &[]) {
            Ok(names) if names.is_empty() => self.notify_tr("No form fields were detected"),
            Ok(names) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(ctx, &info);
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
    pub fn run_console(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let script = self.js_console.input.clone();
        if script.trim().is_empty() {
            return;
        }
        self.js_console.log.push(format!("> {}", script.trim()));
        match self.session.run_javascript(id, &script, None) {
            Ok(o) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(ctx, &info);
                }
                let (error, result) = (o.error.clone(), o.result.clone());
                let out = JsOutput { alerts: o.alerts, console: o.console, requests: o.requests, errors: Vec::new(), doc_errors: Vec::new() };
                self.handle_js(ctx, id, out);
                match error {
                    Some(e) => self.js_console.log.push(format!("Error: {e}")),
                    None => self.js_console.log.extend(result),
                }
            }
            Err(e) => self.js_console.log.push(format!("Error: {e}")),
        }
    }
}
