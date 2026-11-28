//! Script prompts suspend the document worker. The viewer hosts standard questions and menus in
//! the dialog stack and renders custom script controls from each prompt snapshot.
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

fn edit_text_value(value: &serde_json::Value) -> Cow<'_, str> {
    match value {
        serde_json::Value::String(text) => Cow::Borrowed(text),
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => Cow::Owned(value.to_string()),
        _ => Cow::Borrowed(""),
    }
}

fn dialog_values(elements: &[pdfcraft_engine::js::DialogElement], values: &mut std::collections::BTreeMap<String, serde_json::Value>) {
    for e in elements {
        if !e.id.is_empty() && e.kind != "image" {
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
#[derive(Default)]
struct DialogState {
    values: std::collections::BTreeMap<String, serde_json::Value>,
    images: std::collections::HashMap<String, CachedImage>,
    revision: u32,
}

#[derive(Default)]
struct CachedImage {
    revision: u32,
    value: serde_json::Value,
    texture: Option<egui::TextureHandle>,
}

fn custom_elements(
    ui: &mut egui::Ui,
    elements: &[pdfcraft_engine::js::DialogElement],
    all: &[pdfcraft_engine::js::DialogElement],
    state: &mut DialogState,
    focus: &str,
    needs_focus: &mut bool,
    event: &mut Option<String>,
) {
    for e in elements.iter().filter(|e| e.visible) {
        ui.push_id((&e.id, &e.name, &e.kind), |ui| {
            ui.add_enabled_ui(e.enabled, |ui| {
                if e.kind == "image" {
                    let image = state.images.entry(e.id.clone()).or_default();
                    if image.revision != state.revision {
                        image.revision = state.revision;
                        if image.value != e.value {
                            image.value.clone_from(&e.value);
                            image.texture =
                                dialog_image(&e.value).map(|pixels| ui.ctx().load_texture("dialog image", pixels, egui::TextureOptions::LINEAR));
                        }
                    }
                    if let Some(texture) = &image.texture {
                        ui.image((texture.id(), texture.size_vec2()));
                    }
                    return;
                }
                let value = state.values.get(&e.id).cloned().unwrap_or_default();
                match e.kind.as_str() {
                    "view" | "cluster" | "radio_group" | "tab_group" | "tab" => {
                        if e.kind == "cluster" && !e.name.is_empty() {
                            ui.strong(&e.name);
                        }
                        if e.alignment == "align_row" {
                            ui.horizontal_wrapped(|ui| custom_elements(ui, &e.children, all, state, focus, needs_focus, event));
                        } else {
                            custom_elements(ui, &e.children, all, state, focus, needs_focus, event);
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
                        let mut text = edit_text_value(&value).into_owned();
                        let edit =
                            if e.multiline { egui::TextEdit::multiline(&mut text).desired_rows(4) } else { egui::TextEdit::singleline(&mut text) };
                        let response = ui.add(edit.password(e.password).interactive(!e.readonly).char_limit(8 << 20));
                        if *needs_focus && (focus == e.id || focus.is_empty()) {
                            response.request_focus();
                            *needs_focus = false;
                        }
                        // Moving from an unchanged field into a popup must not resume the
                        // script: its next dialog snapshot would close the open dropdown.
                        if response.lost_focus() && text != edit_text_value(&e.value) {
                            *event = Some(e.id.clone());
                        }
                        if response.changed() {
                            state.values.insert(e.id.clone(), text.into());
                        }
                    }
                    "check_box" | "radio" => {
                        let mut checked = value.as_bool().unwrap_or(false);
                        let changed =
                            if e.kind == "radio" { ui.radio(checked, &e.name).clicked() } else { ui.checkbox(&mut checked, &e.name).changed() };
                        if changed {
                            if e.kind == "radio" {
                                radio_peers(all, &e.group, &mut state.values);
                                checked = true;
                            }
                            state.values.insert(e.id.clone(), checked.into());
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
                            let button = ui.button(if label.is_empty() { tl!("Choices") } else { label });
                            ui.ctx().accesskit_node_builder(button.id, |node| node.set_role(egui::accesskit::Role::ComboBox));
                            egui::Popup::from_toggle_button_response(&button).show(|ui| {
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
                            state.values.insert(e.id.clone(), value);
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
                            custom_elements(ui, &e.children, all, state, focus, needs_focus, event);
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

/// The most bytes a text answer may hold; a longer one is not accepted.
const MAX_ANSWER_BYTES: usize = 4096;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// What the open prompt is, so its answer is taken as the right type and a cancellation gives the right reply.
#[derive(Clone, Copy, Debug)]
enum Asked {
    /// A message box (`StandardPrompt`): Proceed answers `proceed`, Alternate answers "3", and Cancel or Escape
    /// answers `on_cancel`, the button set's safe default.
    Alert { proceed: &'static str, on_cancel: &'static str },
    /// A text question or a menu (`ScriptPromptDialog`); a cancellation answers nothing.
    Dialog,
}

// Cancelling a prompt drops its receiver, but a blocked read still occupies its slot.
static ACTIVE_FILE_READERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct FileReaderSlot;

impl FileReaderSlot {
    fn acquire() -> Option<Self> {
        use std::sync::atomic::Ordering::Relaxed;
        const CAPACITY: usize = 2;
        ACTIVE_FILE_READERS.fetch_update(Relaxed, Relaxed, |active| (active < CAPACITY).then_some(active + 1)).ok().map(|_| Self)
    }
}

impl Drop for FileReaderSlot {
    fn drop(&mut self) {
        ACTIVE_FILE_READERS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

type PdfPickResult = Result<Option<pdfcraft_engine::js::SelectedPdf>, String>;

struct PdfPick {
    reply: Sender<PdfPickResult>,
    result: std::sync::mpsc::Receiver<PdfPickResult>,
}

pub(crate) struct Pending {
    run: PendingScript,
    reply: Option<Sender<Option<String>>>,
    custom_prompt: Option<ScriptPrompt>,
    focus: bool,
    serial: u32,
    dialog_state: DialogState,
    progress: Option<pdfcraft_engine::js::Progress>,
    token: std::sync::Arc<()>,
    pdf_pick: Option<PdfPick>,
    file_read: Option<(u32, std::sync::mpsc::Receiver<Result<String, String>>)>,
    dialog: Option<DialogId>,
    asked: Option<Asked>,
    started: Instant,
    zoom: f64,
    page: usize,
}

impl Pending {
    fn accepts_file(&self, id: DocId, serial: u32, token: &std::sync::Arc<()>) -> bool {
        std::sync::Arc::ptr_eq(&self.token, token)
            && self.run.id == id
            && self.serial == serial
            && matches!(self.custom_prompt, Some(ScriptPrompt::File))
            && self.file_read.is_none()
            && self.started.elapsed() < PROMPT_TIMEOUT
    }
    /// Whether this invocation is waiting for an explicit user reply rather than worker execution.
    pub(crate) fn has_prompt(&self) -> bool {
        self.reply.is_some() || self.pdf_pick.is_some()
    }
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
            ScriptPrompt::Alert(_) | ScriptPrompt::Dialog(_) | ScriptPrompt::File => return None,
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
    if matches!(pending.custom_prompt.take(), Some(ScriptPrompt::File))
        && pending.file_read.as_ref().is_some_and(|(serial, _)| *serial == pending.serial)
    {
        pending.file_read = None;
    }
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
    pub(crate) fn show_document_tool_buttons(&mut self, ui: &mut egui::Ui) {
        let Some((_, id)) = self.active_ids() else { return };
        let buttons = self.session.get(id).map(|d| d.script_tool_buttons().to_vec()).unwrap_or_default();
        if buttons.is_empty() {
            return;
        }
        let enabled = self.session.javascript() && !self.modal_open() && self.script_pending.is_none();
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
        let (zoom, page, highlight) = (view.zoom(), view.current(), view.session.highlight_fields);
        self.session.set_javascript_view(id, zoom * 100.0, highlight, page);
        match start(&mut self.session) {
            Ok(run) => {
                self.script_pending = Some(Pending {
                    run,
                    reply: None,
                    custom_prompt: None,
                    dialog: None,
                    asked: None,
                    started: Instant::now(),
                    zoom,
                    page,
                    focus: false,
                    serial: 0,
                    dialog_state: Default::default(),
                    progress: None,
                    token: std::sync::Arc::new(()),
                    file_read: None,
                    pdf_pick: None,
                });
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
        if pending.has_prompt() && pending.started.elapsed() >= PROMPT_TIMEOUT {
            self.close_script_prompt(ctx, &pending);
            self.session.timeout_javascript(pending.run);
            self.notify("The form script timed out; its result was discarded");
            return;
        }
        if let Some(pick) = pending.pdf_pick.as_ref() {
            let result = match pick.result.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Disconnected) => Some(Ok(None)),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result
                && let Some(pick) = pending.pdf_pick.take()
            {
                if let Err(error) = &result {
                    self.notify(tl!(error));
                }
                pending.started = Instant::now();
                let _ = pick.reply.send(result);
            }
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
                if pending.started.elapsed() < PROMPT_TIMEOUT && serial == pending.serial && matches!(pending.custom_prompt, Some(ScriptPrompt::File))
                {
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
        if let (Some(id), Some(asked)) = (pending.dialog, pending.asked)
            && let Some(result) = self.take_prompt_result(id, asked)
        {
            let (answer, cancel) = match result {
                DialogResult::Ok(Some(answer)) => (Some(answer), false),
                _ => (None, true),
            };
            reply_to_prompt(ctx, &mut pending, answer, cancel);
        }
        while !pending.has_prompt() {
            match pending.run.receiver.try_recv() {
                Ok(InteractiveEvent::Progress(progress)) => {
                    pending.progress = Some(progress);
                    ctx.request_repaint();
                }
                Ok(InteractiveEvent::SelectPdf(_)) if pending.started.elapsed() >= pending.run.timeout => break,
                Ok(InteractiveEvent::SelectPdf(reply)) => {
                    let target = self.pick_target(Some(pending.run.id));
                    let (send, result) = std::sync::mpsc::channel();
                    pending.started = Instant::now();
                    pending.pdf_pick = Some(PdfPick { reply, result });
                    let request = craft::host::PickRequest::open("script-source-pdf")
                        .title(tl!("Select a source PDF for this document"))
                        .filter(tl!("PDF documents"), &["pdf"]);
                    self.pick_file(request, target, move |_, ctx, picked| {
                        let Some(path) = picked.first_path() else {
                            let _ = send.send(Ok(None));
                            return;
                        };
                        let Some(slot) = FileReaderSlot::acquire() else {
                            let _ = send.send(Err("The selected-file reader is busy".into()));
                            return;
                        };
                        let wake = ctx.clone();
                        let report = send.clone();
                        if std::thread::Builder::new()
                            .name("pdfcraft-script-source".into())
                            .spawn(move || {
                                let _slot = slot;
                                let _ = send.send(pdfcraft_engine::js::read_selected_pdf(&path).map(Some));
                                wake.request_repaint();
                            })
                            .is_err()
                        {
                            let _ = report.send(Err("Couldn't start the selected-file reader".into()));
                        }
                    });
                }
                Ok(InteractiveEvent::Prompt(_, _)) if pending.started.elapsed() >= pending.run.timeout => break,
                Ok(InteractiveEvent::Prompt(prompt @ (ScriptPrompt::Dialog(_) | ScriptPrompt::File), reply)) => {
                    pending.dialog_state.values.clear();
                    if let ScriptPrompt::Dialog(dialog) = &prompt {
                        dialog_values(&dialog.elements, &mut pending.dialog_state.values);
                    } else {
                        pending.dialog_state.images.clear();
                    }
                    pending.serial = pending.serial.saturating_add(1);
                    pending.dialog_state.revision = pending.serial;
                    pending.focus = true;
                    pending.started = Instant::now();
                    pending.custom_prompt = Some(prompt);
                    pending.reply = Some(reply);
                }
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
                    if out.completed_at.unwrap_or_else(Instant::now).saturating_duration_since(pending.started) >= pending.run.timeout {
                        break;
                    }
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
                                    alerts: out.post_commit_alerts,
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
                Err(TryRecvError::Disconnected) if pending.started.elapsed() >= pending.run.timeout => break,
                Err(TryRecvError::Disconnected) => {
                    self.session.discard_javascript(&pending.run);
                    self.notify("The form script stopped unexpectedly");
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if !pending.has_prompt() && pending.started.elapsed() >= pending.run.timeout {
            self.close_script_prompt(ctx, &pending);
            self.session.timeout_javascript(pending.run);
            self.notify("The form script timed out; its result was discarded");
            return;
        }
        // Input events wake egui immediately; idle prompts only need a coarse timeout check.
        frame::request_repaint_after(
            ctx,
            if pending.has_prompt() && pending.file_read.is_none() && pending.pdf_pick.is_none() { 1.0 } else { 0.016 },
        );
        self.script_pending = Some(pending);
    }
    /// Display the current typed prompt and send one answer/cancellation without replaying script side effects.
    pub(crate) fn show_script_dialog(&mut self, ctx: &egui::Context) {
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
        let Some(prompt) = pending.custom_prompt.as_ref() else { return };
        let Some(doc) = self.session.get(pending.run.id) else { return };
        let document_name = document_name(&doc.name);
        let mut answer = None;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut choose_file = false;
        let prompt_modal = egui::Modal::new(egui::Id::unique(("document-script-prompt", pending.run.id.0, pending.serial)));
        // A different dialog may have been opened after this prompt. Drawing last alone
        // does not reorder retained egui areas, so explicitly retain modal input priority.
        ctx.move_to_top(prompt_modal.area.layer());
        let modal = prompt_modal.show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(tl!("This document's script is asking"));
            ui.add(egui::Label::new(crate::i18n::fmt(tl!("Document: {name}"), &[("name", &document_name)])).truncate());
            ui.separator();
            match prompt {
                ScriptPrompt::File => {
                    ui.label("Select a UTF-8 text file to share with this document's script.");
                    choose_file = ui.add_enabled(pending.file_read.is_none(), egui::Button::new(tl!("Open"))).clicked();
                }
                ScriptPrompt::Dialog(dialog) => {
                    ui.label(&dialog.title);
                    let mut event = None;
                    egui::ScrollArea::vertical().max_height(600.0).show(ui, |ui| {
                        custom_elements(
                            ui,
                            &dialog.elements,
                            &dialog.elements,
                            &mut pending.dialog_state,
                            &dialog.focus,
                            &mut pending.focus,
                            &mut event,
                        );
                    });
                    let revision = pending.dialog_state.revision;
                    pending.dialog_state.images.retain(|_, image| image.revision == revision);
                    if let Some(event) = event {
                        let mut changed = serde_json::Map::new();
                        dialog_changes(&dialog.elements, &pending.dialog_state.values, &mut changed);
                        answer = Some(serde_json::json!({"event":event,"values":changed}).to_string());
                    }
                }
                _ => {}
            }
            if ui.button(tl!("Cancel")).clicked() {
                cancel = true;
            }
        });
        cancel |= answer.is_none() && modal.should_close();
        if answer.is_some() || cancel {
            reply_to_prompt(ctx, pending, answer, cancel);
        }
        if choose_file && !cancel {
            let id = pending.run.id;
            let serial = pending.serial;
            let token = pending.token.clone();
            let target = self.pick_target(Some(id));
            self.pick_first(craft::host::PickRequest::open("script-text-file"), target, move |app, ctx, path| {
                let Some(pending) = app.script_pending.as_mut().filter(|p| p.accepts_file(id, serial, &token)) else {
                    return;
                };
                let Some(slot) = FileReaderSlot::acquire() else {
                    app.notify(tl!("The selected-file reader is busy"));
                    return;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                let wake = ctx.clone();
                let worker = std::thread::Builder::new().name("pdfcraft-script-file".into()).spawn(move || {
                    use std::io::Read;
                    let _slot = slot;
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
                    reply_to_prompt(ctx, pending, None, true);
                    app.notify("Couldn't start the selected-file reader");
                }
            });
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn file_prompt_identity_and_completion_reject_stale_results() {
        let mut app = PdfCraftApp::for_unit_tests();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes(&egui::Context::default(), "synthetic.pdf", None, bytes.to_vec()).unwrap();
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
                let mut p = app.script_pending.take().unwrap();
                app.session.discard_javascript(&p.run);
                // The replacement realm shares the old worker's execution slot.
                let (tx, rx) = std::sync::mpsc::channel();
                p.file_read = Some((p.serial, rx));
                reply_to_prompt(&ctx, &mut p, None, true);
                assert!(p.file_read.is_none(), "cancelling releases even an unfinished read");
                assert!(tx.send(Ok("cancelled".into())).is_err());
                assert!(matches!(p.run.receiver.recv_timeout(Duration::from_secs(5)), Ok(InteractiveEvent::Complete(_))));
            }
        }
    }

    #[test]
    fn opening_a_dialog_dropdown_only_commits_changed_text() {
        for (initial, edited) in [("\"1\"", false), ("1", false), ("true", false), ("null", false), ("1", true)] {
            let mut app = PdfCraftApp::for_unit_tests();
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes(&egui::Context::default(), "synthetic.pdf", None, bytes.to_vec()).unwrap();
            let id = app.active_ids().unwrap().1;
            app.start_ui_script(
                id,
                &r#"app.execDialog({
                    initialize: function(d) { d.load({text: INITIAL, pick: {Fighter: 1, Wizard: -2}}); d.focus('text'); },
                    description: {name: 'Choose class', elements: [
                        {type: 'edit_text', item_id: 'text'},
                        {type: 'popup', item_id: 'pick'}
                    ]}
                });"#
                    .replace("INITIAL", initial),
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
            let pending = app.script_pending.as_ref().unwrap();
            assert!(pending.has_prompt());
            let loaded = pending.dialog_state.values["text"].clone();
            let mut h = Harness::builder()
                .with_size(egui::vec2(800.0, 600.0))
                .with_accessibility_check(false)
                .build_ui_state(|ui, app: &mut PdfCraftApp| app.show_script_dialog(ui.ctx()), app);
            h.run();
            assert_eq!(h.state().script_pending.as_ref().unwrap().dialog_state.values["text"], loaded);
            if edited {
                h.get_by_role(egui::accesskit::Role::TextInput).type_text("2");
                h.run();
                assert!(h.state().script_pending.as_ref().unwrap().dialog_state.values["text"].is_string());
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
    #[test]
    fn polling_after_the_deadline_accepts_an_on_time_completion_behind_progress() {
        let ctx = egui::Context::default();
        let (mut app, id) = app(&ctx);
        app.start_ui_script(id, "1 + 1;", None, None);
        let pending = app.script_pending.as_mut().unwrap();
        let InteractiveEvent::Complete(mut out) = pending.run.receiver.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("expected the script to finish");
        };
        pending.started = Instant::now() - pending.run.timeout - Duration::from_secs(1);
        out.completed_at = Some(pending.started + pending.run.timeout / 2);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(InteractiveEvent::Progress(pdfcraft_engine::js::Progress {
            text: String::new(),
            value: 0.0,
            duration: 0.0,
            active: false,
            cancel: Default::default(),
        }))
        .unwrap();
        tx.send(InteractiveEvent::Complete(out)).unwrap();
        pending.run.receiver = rx;
        app.poll_script_dialog(&ctx);
        assert!(app.script_pending.is_none());
        assert!(app.posted_notices.is_empty());
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
                Ok(InteractiveEvent::SelectPdf(_)) => panic!("the script requested an unexpected PDF"),
                Ok(InteractiveEvent::Complete(_)) | Err(TryRecvError::Disconnected) => panic!("the script ended without asking"),
                Ok(InteractiveEvent::Progress(_)) | Err(TryRecvError::Empty) => {
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
