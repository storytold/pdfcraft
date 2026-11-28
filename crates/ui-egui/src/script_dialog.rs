//! Script prompts suspend the document worker, never the egui event loop. Each prompt is a hosted dialog
//! (`app.dialogs`) whose answer `poll_script_dialog` takes by id and sends back to the waiting worker.
use crate::PdfCraftApp;
use craft::behavior::{DisplayOrder, ItemSource, Selection};
use craft::theme::variant::Tone;
use craft::ui::contract::{ActivateOn, Bind, CollectionEvent, Props as _, SelectMode, Show as _, ShowCollection as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, Prompt, PromptChoice, StackDialog, StandardPrompt, dialog_id};
use craft::ui::display::{Elide, Text};
use craft::ui::list::{List, ListRow, OpenSet};
use craft::ui::runtime::frame;
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::text::TextField;
use pdfcraft_engine::{
    DocId,
    js::{InteractiveEvent, JsOutput, MenuItem, Message, MessageButtons, PendingScript, Prompt as ScriptPrompt},
};
use std::borrow::Cow;
use std::sync::mpsc::{Sender, TryRecvError};
use std::time::{Duration, Instant};

/// The most bytes a text answer may hold; a longer one is not accepted.
const MAX_ANSWER_BYTES: usize = 4096;

/// What the open prompt is, so its answer is taken as the right type and a cancellation gives the right reply.
#[derive(Clone, Copy, Debug)]
enum Asked {
    /// A message box (`StandardPrompt`): Proceed answers `proceed`, Alternate answers "3", and Cancel or Escape
    /// answers `on_cancel`, the button set's safe default.
    Alert { proceed: &'static str, on_cancel: &'static str },
    /// A text question or a menu (`ScriptPromptDialog`); a cancellation answers nothing.
    Dialog,
}

pub(crate) struct Pending {
    run: PendingScript,
    reply: Option<Sender<Option<String>>>,
    dialog: Option<DialogId>,
    asked: Option<Asked>,
    started: Instant,
    zoom: f64,
    page: usize,
}

/// The document's name as a prompt shows it: document data, so control and direction characters go and it is capped.
fn document_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .take(160)
        .collect()
}

/// The line naming the document a prompt comes from.
fn document_line(document: &str) -> String {
    crate::i18n::fmt(tl!("Document: {name}"), &[("name", document)]).to_string()
}

/// The caption every script prompt carries, so the document's own title cannot pass for the app's.
fn caption() -> String {
    tl!("This document's script is asking").to_string()
}

/// A script message box with the button set the script asked for.
fn alert_prompt(run: DocId, document: &str, message: &Message) -> (StandardPrompt, Asked) {
    let yes_no = matches!(message.buttons, MessageButtons::YesNo | MessageButtons::YesNoCancel);
    let on_cancel = match message.buttons {
        MessageButtons::Ok => "1",
        MessageButtons::YesNo => "3",
        _ => "2",
    };
    let mut prompt = StandardPrompt::custom(message.text.clone()).title(caption()).paragraph(document_line(document));
    if let Some(title) = message.title.as_ref().filter(|t| !t.is_empty()) {
        prompt = prompt.paragraph(title.clone());
    }
    prompt = prompt.button(PromptChoice::Proceed, if yes_no { tl!("Yes").to_string() } else { tl!("OK").to_string() });
    if yes_no {
        // Production's order: No between Cancel and Yes.
        prompt = prompt.button(PromptChoice::Alternate, tl!("No").to_string()).alternate_role(craft::ui::sections::DialogRole::Alternative);
    }
    // One script runs at a time and its prompt closes before the next opens, so the run names the instance.
    let prompt = prompt.button(PromptChoice::Cancel, tl!("Cancel").to_string()).instance(("script.prompt", run.0));
    (prompt, Asked::Alert { proceed: if yes_no { "4" } else { "1" }, on_cancel })
}

/// One row of a menu prompt's tree, in the order a depth-first walk reaches it.
struct Node {
    name: String,
    value: String,
    enabled: bool,
    depth: usize,
    group: bool,
}

/// The menu `items` as rows in depth-first order, keyed by their place in `out`. The engine bounds a menu's depth.
fn flatten(items: &[MenuItem], depth: usize, out: &mut Vec<Node>) {
    for item in items {
        out.push(Node { name: item.name.clone(), value: item.value.clone(), enabled: item.enabled, depth, group: !item.children.is_empty() });
        flatten(&item.children, depth + 1, out);
    }
}

/// The rows of `nodes` that show: those whose groups above them are all open.
fn shown_nodes(nodes: &[Node], open: &OpenSet<usize>) -> Vec<usize> {
    let mut shown = Vec::new();
    let mut closed_at: Option<usize> = None;
    for (at, node) in nodes.iter().enumerate() {
        if closed_at.is_some_and(|depth| node.depth > depth) {
            continue;
        }
        closed_at = (node.group && !open.is_open(&at)).then_some(node.depth);
        shown.push(at);
    }
    shown
}

/// The shown rows of a menu as the display order of a tree list.
struct MenuTree<'a> {
    nodes: &'a [Node],
    shown: &'a [usize],
    open: &'a OpenSet<usize>,
}

impl MenuTree<'_> {
    fn node(&self, at: usize) -> Option<&Node> {
        self.shown.get(at).and_then(|key| self.nodes.get(*key))
    }
}

impl DisplayOrder<usize> for MenuTree<'_> {
    fn len(&self) -> usize {
        self.shown.len()
    }

    fn key(&self, at: usize) -> usize {
        self.shown.get(at).copied().unwrap_or_default()
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        self.shown.iter().position(|node| node == key)
    }

    fn depth(&self, at: usize) -> usize {
        self.node(at).map_or(0, |node| node.depth)
    }

    fn has_children(&self, at: usize) -> bool {
        self.node(at).is_some_and(|node| node.group)
    }

    fn open(&self, at: usize) -> bool {
        self.shown.get(at).is_some_and(|key| self.open.is_open(key))
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        self.node(at).map(|node| Cow::Borrowed(node.name.as_str()))
    }
}

impl ItemSource<usize> for MenuTree<'_> {}

/// What a `ScriptPromptDialog` asks.
enum Body {
    /// A text question; `text` is the answer being typed.
    Text { title: String, question: String, label: String, password: bool, text: String },
    /// A menu: only an enabled leaf answers.
    Menu { nodes: Vec<Node>, open: OpenSet<usize>, selection: Selection<usize> },
}

/// A script's text question or menu, over the app's own caption and the document's name. Its answer is the typed
/// text or the chosen leaf's value; cancelling gives none.
pub(crate) struct ScriptPromptDialog {
    id: DialogId,
    document: String,
    body: Body,
}

impl ScriptPromptDialog {
    /// The dialog for `prompt` of document `run`, or `None` for a message box, which the standard prompt answers.
    fn for_prompt(run: DocId, document: String, prompt: ScriptPrompt) -> Option<Self> {
        let body = match prompt {
            ScriptPrompt::Text { title, question, password, label, default, .. } => Body::Text { title, question, label, password, text: default },
            ScriptPrompt::Menu(items) => {
                let mut nodes = Vec::new();
                flatten(&items, 0, &mut nodes);
                Body::Menu { nodes, open: OpenSet::new(), selection: Selection::default() }
            }
            ScriptPrompt::Alert(_) => return None,
        };
        Some(ScriptPromptDialog { id: dialog_id(("script.prompt", run.0)), document, body })
    }
}

impl StackDialog for ScriptPromptDialog {
    type Result = Option<String>;

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut egui::Ui, _: &DialogStack) -> Option<DialogResult<Option<String>>> {
        let Self { document, body, .. } = self;
        let mut prompt = match &*body {
            Body::Text { question, .. } => Prompt::new(caption(), question.clone()),
            Body::Menu { .. } => Prompt::new(caption(), String::new()),
        };
        let accepts = matches!(body, Body::Text { .. });
        if let Body::Text { title, label, password, .. } = &*body {
            if *password {
                prompt = prompt.tone(
                    Tone::Warn,
                    tl!("This request comes from the document, not PdfCraft. Only enter a password if you trust this document.").to_string(),
                );
            }
            if !title.is_empty() {
                prompt = prompt.paragraph(title.clone());
            }
            if !label.is_empty() {
                prompt = prompt.paragraph(label.clone());
            }
        }
        let mut answers = Vec::new();
        if accepts {
            answers.push(FooterButton::new(tl!("OK").to_string(), DialogRole::Accept));
        }
        answers.push(FooterButton::new(tl!("Cancel").to_string(), DialogRole::Cancel));
        let line = document_line(document);
        let mut chosen: Option<String> = None;
        // PLAN: the text field takes no initial-focus prop; the user clicks it or tabs to it.
        let out = prompt
            .answers(answers)
            .extras(|ui| {
                Text::new(line.clone()).elide(Elide::End).show(ui);
                match &mut *body {
                    Body::Text { text, password, .. } => {
                        let field = TextField::new(tl!("Answer").to_string(), Bind::bound(&mut *text)).limit(MAX_ANSWER_BYTES);
                        if *password {
                            field.masked().show(ui);
                        } else {
                            field.show(ui);
                        }
                    }
                    Body::Menu { nodes, open, selection } => {
                        let shown = shown_nodes(nodes, open);
                        let groups: Vec<usize> = nodes.iter().enumerate().filter(|(_, node)| node.group).map(|(at, _)| at).collect();
                        let tree = MenuTree { nodes, shown: &shown, open };
                        let listed = List::<usize>::new(tl!("Choices").to_string())
                            .tree(true)
                            .natural_height(true)
                            .select(SelectMode::Single)
                            .activate(ActivateOn::Click)
                            .selection(Bind::bound(&mut *selection))
                            .show_items(ui, &tree, &mut |key, _| match tree.nodes.get(*key) {
                                Some(node) if node.name == "-" => ListRow::spacer(),
                                Some(node) => {
                                    let row = ListRow::new(node.name.clone()).depth(node.depth).enabled(node.enabled);
                                    if node.group { row.group(tree.open.is_open(key)) } else { row }
                                }
                                None => ListRow::new(String::new()),
                            });
                        for event in &listed.events {
                            match event {
                                CollectionEvent::Activate(key) => {
                                    if let Some(node) = nodes.get(*key) {
                                        if node.group {
                                            let now = open.is_open(key);
                                            open.set(key, !now);
                                        } else if node.enabled && node.name != "-" {
                                            chosen = Some(node.value.clone());
                                        }
                                    }
                                }
                                CollectionEvent::Open(change) => open.apply(&change.key, change.open, change.all, &groups),
                                _ => {}
                            }
                        }
                    }
                }
            })
            .show(ui);
        if let Some(value) = chosen {
            return Some(DialogResult::Ok(Some(value)));
        }
        match out.picked {
            Some(0) if accepts => match &*body {
                Body::Text { text, .. } if text.len() <= MAX_ANSWER_BYTES => Some(DialogResult::Ok(Some(text.clone()))),
                _ => None,
            },
            Some(_) => Some(DialogResult::Cancel),
            None => None,
        }
    }
}

/// Consume the current prompt before replying so two UI events cannot resume it twice.
fn reply_to_prompt(ctx: &egui::Context, pending: &mut Pending, answer: Option<String>, cancel: bool) {
    if let Some(reply) = pending.reply.take() {
        let answer = if cancel {
            match pending.asked {
                Some(Asked::Alert { on_cancel, .. }) => Some(on_cancel.to_owned()),
                _ => None,
            }
        } else {
            answer
        };
        let _ = reply.send(answer);
    }
    pending.dialog = None;
    pending.asked = None;
    pending.started = Instant::now();
    frame::request_repaint_after(ctx, 0.0);
}

impl PdfCraftApp {
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
        let (zoom, page, highlight) = (view.zoom(), view.current(), view.session.highlight_fields);
        self.session.set_javascript_view(id, zoom * 100.0, highlight, page);
        match start(&mut self.session) {
            Ok(run) => {
                self.script_pending = Some(Pending { run, reply: None, dialog: None, asked: None, started: Instant::now(), zoom, page });
            }
            Err(e) => self.notify(e.to_string()),
        }
    }

    /// Takes the answer of the hosted prompt `id` that `asked` describes, as the reply the worker waits for: the
    /// answer, or a cancellation.
    fn take_prompt_result(&mut self, id: DialogId, asked: Asked) -> Option<DialogResult<Option<String>>> {
        match asked {
            Asked::Alert { proceed, .. } => {
                self.dialogs.take_by_id::<StandardPrompt>(id).map(|result| match result.ok().map(|answer| answer.choice) {
                    Some(PromptChoice::Proceed) => DialogResult::Ok(Some(proceed.to_owned())),
                    Some(PromptChoice::Alternate) => DialogResult::Ok(Some("3".to_owned())),
                    _ => DialogResult::Cancel,
                })
            }
            Asked::Dialog => self.dialogs.take_by_id::<ScriptPromptDialog>(id),
        }
    }

    /// Closes the prompt `pending` has open, before its run is dropped; the cancellation it leaves is taken so it
    /// does not linger in the host.
    fn close_script_prompt(&mut self, ctx: &egui::Context, pending: &Pending) {
        if let (Some(id), Some(asked)) = (pending.dialog, pending.asked) {
            self.dialogs.cancel_by_id(ctx, id);
            let _ = self.take_prompt_result(id, asked);
        }
    }

    /// Opens the hosted dialog for `prompt` above any open dialog and returns its id and what it answers with.
    fn open_script_prompt(&mut self, run: DocId, prompt: ScriptPrompt) -> Option<(DialogId, Asked)> {
        let document = self.session.get(run).map(|d| document_name(&d.name)).unwrap_or_default();
        if let ScriptPrompt::Alert(message) = &prompt {
            let (alert, asked) = alert_prompt(run, &document, message);
            // PLAN: assumed `dialogs::host::open_over(app: &mut PdfCraftApp, dialog: D) -> DialogId` (pdf-hub-w2)
            return Some((crate::dialogs::host::open_over(self, alert), asked));
        }
        let dialog = ScriptPromptDialog::for_prompt(run, document, prompt)?;
        Some((crate::dialogs::host::open_over(self, dialog), Asked::Dialog))
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
            && self.views.iter().find(|v| v.id == pending.run.id).is_some_and(|v| v.zoom() == pending.zoom && v.current() == pending.page);
        if !current {
            self.close_script_prompt(ctx, &pending);
            self.session.discard_javascript(&pending.run);
            return;
        }
        if (pending.dialog.is_none() && pending.started.elapsed() > Duration::from_secs(10))
            || (pending.dialog.is_some() && pending.started.elapsed() > Duration::from_secs(300))
        {
            self.close_script_prompt(ctx, &pending);
            self.session.timeout_javascript(pending.run);
            self.notify("The form script timed out; its result was discarded");
            return;
        }
        if let (Some(id), Some(asked)) = (pending.dialog, pending.asked)
            && let Some(result) = self.take_prompt_result(id, asked)
        {
            let (answer, cancel) = match result {
                DialogResult::Ok(Some(answer)) => (Some(answer), false),
                _ => (None, true),
            };
            reply_to_prompt(ctx, &mut pending, answer, cancel);
        }
        if pending.dialog.is_none() {
            match pending.run.receiver.try_recv() {
                Ok(InteractiveEvent::Prompt(prompt, reply)) => match self.open_script_prompt(pending.run.id, prompt) {
                    Some((dialog, asked)) => {
                        pending.reply = Some(reply);
                        pending.dialog = Some(dialog);
                        pending.asked = Some(asked);
                        pending.started = Instant::now();
                    }
                    // Dropping `reply` cancels the question for the worker.
                    None => drop(reply),
                },
                Ok(InteractiveEvent::Complete(out)) => {
                    let id = pending.run.id;
                    match self.session.finish_javascript(pending.run, out) {
                        Ok(out) => {
                            if let Some(i) = self.views.iter().position(|v| v.id == id)
                                && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                            {
                                self.views[i].document_changed(ctx, &info);
                            }
                            self.handle_js(
                                ctx,
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
        frame::request_repaint_after(ctx, if pending.dialog.is_some() { 1.0 } else { 0.016 });
        self.script_pending = Some(pending);
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    /// The central shortcut/page-input gate must see a real hosted script prompt.
    #[test]
    fn a_script_prompt_blocks_document_input_until_answered() {
        let ctx = &egui::Context::default();
        let mut app = PdfCraftApp::for_unit_tests();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes(ctx, "synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        app.start_ui_script(id, "app.response({cQuestion:'Wait'});", None, None);
        let ctx = egui::Context::default();
        for _ in 0..300 {
            app.poll_script_dialog(&ctx);
            if app.script_pending.as_ref().is_some_and(|p| p.dialog.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let dialog = app.script_pending.as_ref().unwrap().dialog.expect("the prompt opened");
        assert!(app.modal_open(), "an unanswered script prompt must block document input");
        assert!(app.dialogs.stack().contains(dialog), "the prompt is a hosted dialog");
        assert!(app.dialogs.cancel_by_id(&ctx, dialog));
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
    use craft::host::{AppInfo, Host, Picked, ScriptedFiles, Wake};
    use craft::platform::clipboard::{Clipboard, EventClipboard};
    use craft::platform::open::FileIntake;
    use craft::platform::watch::FileWatch;
    use craft::ui::notices::PostedNotice;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Create a native app over `files` with a visible synthetic blank document and no renderer.
    fn app_over(ctx: &egui::Context, files: Arc<ScriptedFiles>) -> (PdfCraftApp, DocId) {
        let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).files(files).build();
        let (intake, _) = FileIntake::new(Wake::none());
        let clipboard = Clipboard::new(EventClipboard::new(|| 0, |_| Ok(0)));
        let services = crate::AppServices::new(Wake::none(), clipboard, None, None, intake, FileWatch::new(Wake::none()).inline());
        let mut app = PdfCraftApp::with_host(Arc::new(host), services);
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        let bytes = app.session.create_blank(200.0, 300.0, 2).unwrap();
        app.open_bytes(ctx, "synthetic.pdf", None, bytes.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        (app, id)
    }
    fn app(ctx: &egui::Context) -> (PdfCraftApp, DocId) {
        app_over(ctx, Arc::new(ScriptedFiles::new()))
    }
    /// Refused clicks and execution/prompt timeouts are visible, while stale runs stay silent.
    #[test]
    fn busy_and_timeout_notices_do_not_claim_worker_termination() {
        let ctx = egui::Context::default();
        for (timeout, prompt) in [(true, false), (true, true), (false, false)] {
            let (mut app, id) = app(&ctx);
            app.start_ui_script(id, "app.response({cQuestion:'Wait'});", None, None);
            app.start_ui_script(id, "var shouldNotRun=1;", None, None);
            assert!(app.posted_notices.contains(&PostedNotice::new("Another form script is active; this click was not run")));
            app.posted_notices.clear();
            if prompt {
                let deadline = Instant::now() + Duration::from_secs(5);
                while app.script_pending.as_ref().unwrap().dialog.is_none() && Instant::now() < deadline {
                    app.poll_script_dialog(&ctx);
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(app.script_pending.as_ref().unwrap().dialog.is_some());
            }
            if timeout {
                app.script_pending.as_mut().unwrap().started = Instant::now() - Duration::from_secs(if prompt { 301 } else { 11 });
            } else {
                app.views.iter_mut().find(|v| v.id == id).unwrap().view.zoom += 1.0;
            }
            app.poll_script_dialog(&ctx);
            assert!(app.script_pending.is_none());
            if timeout {
                assert_eq!(app.posted_notices, [PostedNotice::new("The form script timed out; its result was discarded")]);
            } else {
                assert!(app.posted_notices.is_empty());
            }
        }
    }
    /// Comparison completion preserves globals and sends Save As/Print to its source document.
    #[test]
    fn non_active_comparison_document_completes_native_script() {
        let ctx = &egui::Context::default();
        for request in ["", "app.execMenuItem('SaveAs');", "print();"] {
            let saved = PathBuf::from("/scripted/source.pdf");
            let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![saved.clone()])]));
            let (mut app, old) = app_over(ctx, files.clone());
            app.views.iter_mut().find(|v| v.id == old).unwrap().view.current = 1;
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes(ctx, "new.pdf", None, bytes.to_vec()).unwrap();
            let new = app.active_ids().unwrap().1;
            assert_ne!(new, old);
            let new_before = app.session.save_bytes(new).unwrap();
            app.compare_old = Some(old);
            app.run_compare(&egui::Context::default());
            assert!(app.compare.as_ref().unwrap().side_by_side);
            app.printer_source = Some(std::sync::Arc::new(Vec::new));
            app.start_ui_script(old, &format!("var comparisonKept=42;{request}"), None, None);
            let ctx = egui::Context::default();
            let deadline = Instant::now() + Duration::from_secs(12);
            while app.script_pending.is_some() && Instant::now() < deadline {
                app.poll_script_dialog(&ctx);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.script_pending.is_none());
            // The Save As the script asked for takes the queued answer on this pass.
            app.save_flow_pass(&ctx);
            assert_eq!(app.session.run_javascript(old, "comparisonKept", None).unwrap().result.as_deref(), Some("42"));
            assert_eq!(app.session.save_bytes(new).unwrap(), new_before);
            assert!(app.session.get(new).unwrap().path.is_none());
            if request.contains("SaveAs") {
                assert_eq!(app.active_ids().unwrap().1, new, "Save As need not change the comparison tab");
                assert_eq!(app.session.get(old).unwrap().path.as_deref(), saved.to_str());
                let mut reopened = pdfcraft_engine::Session::new();
                let id = reopened.open("saved.pdf", None, std::sync::Arc::new(files.file(&saved).unwrap()), None).unwrap();
                assert_eq!(reopened.get(id).unwrap().info.pages.len(), 2, "the saved file is the two-page source, not the one-page active file");
            } else if request == "print();" {
                assert_eq!(app.active_ids().unwrap().1, old, "Print must retain its source as the active document");
                assert!(app.dialogs.is_open::<craft::print::ui::HostedPrint>());
                assert_eq!(app.print_draft.current_page, 1);
                assert!(files.file(&saved).is_none());
            } else {
                assert_eq!(app.active_ids().unwrap().1, new);
                assert!(files.file(&saved).is_none());
            }
        }
    }
    /// Script focus reveals the requested field page before opening its editor.
    #[test]
    fn script_focus_reveals_an_offscreen_widget() {
        let _ctx = &egui::Context::default();
        let ctx = &egui::Context::default();
        let (mut app, id) = app(ctx);
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
        app.handle_js(ctx, id, JsOutput { requests: vec![pdfcraft_engine::js::Request::Focus("later".into())], ..Default::default() });
        let view = app.views.iter().find(|v| v.id == id).unwrap();
        assert_eq!(view.current(), 1);
        assert_eq!(view.session.forms.focus.as_ref().unwrap().name, "later");
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
        let _ctx = &egui::Context::default();
        let ctx = &egui::Context::default();
        // An absolute path on this system (the document has no folder): on Windows, Acrobat's
        // `/C/…` form, which reads as `C:/…`.
        let path = if cfg!(windows) { "/C/abs/other.pdf" } else { "/abs/other.pdf" };
        let script = format!("getField\\('log'\\).value = app.popUpMenuEx\\({{cName:'A', cReturn:'a'}}\\) + event.type; app.openDoc\\('{path}'\\);");
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(ctx, "link.pdf", None, linked(&script)).unwrap();
        let id = app.active_ids().unwrap().1;
        app.run_link_script(ctx, id, 0, 0);
        // The worker's question is read from its reply channel, before any pass hosts it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let (prompt, reply) = loop {
            match app.script_pending.as_ref().unwrap().run.receiver.try_recv() {
                Ok(InteractiveEvent::Prompt(prompt, reply)) => break (prompt, reply),
                Ok(InteractiveEvent::Complete(_)) | Err(TryRecvError::Disconnected) => panic!("the script ended without asking"),
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "the script did not ask");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert!(matches!(&prompt, ScriptPrompt::Menu(items) if items.len() == 1 && items[0].value == "a"), "{prompt:?}");
        reply.send(Some("a".into())).unwrap();
        let ctx = egui::Context::default();
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
