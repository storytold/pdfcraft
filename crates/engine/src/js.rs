//! Acrobat JavaScript in documents: field scripts run during form edits ([`JsRunner`], wired
//! into `forms` through its [`pdfcraft_forms::Scripts`] hook), and button actions and console
//! input run on request ([`Session::run_javascript`]). What scripts ask of the viewer (alerts,
//! printing, navigation, links) is kept for the front end in [`JsOutput`].
//!
//! Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript is [`Session::set_javascript`]; with it
//! off, scripts don't run and AF calls keep working natively.

use pdfcraft_forms::{Field, FieldChange, FieldEvent, FieldKind, FieldProps, ScriptResult, Scripts};
pub use pdfcraft_js::{Limits, Message, MessageButtons, MessageIcon, Outcome, Panel, Request};

use crate::{DocId, Edit, EditError, Session};

/// Reset writeback does two full passes (preview and commit); reject amplification before either.
const MAX_SCRIPT_RESETS: usize = pdfcraft_js::MAX_RESET_CALLS;
const MAX_SCRIPT_RESET_SELECTORS: usize = 4096;
const MAX_SCRIPT_RESET_WORK: usize = 1 << 24;

/// Bound request count, selector storage/scans and estimated form/calculation visits before cloning a PDF.
fn validate_reset_work(fields: &[Field], states: &[pdfcraft_js::FieldState], requests: &[Request]) -> Result<(), EditError> {
    let invalid = || EditError::Invalid("script reset work limit exceeded; use fewer reset calls or combine field selections".into());
    let (mut resets, mut selectors, mut bytes) = (0usize, 0usize, 0usize);
    for request in requests {
        if let Request::Reset(names) = request {
            resets = resets.saturating_add(1);
            selectors = selectors.saturating_add(names.len());
            if resets > MAX_SCRIPT_RESETS || selectors > MAX_SCRIPT_RESET_SELECTORS {
                return Err(invalid());
            }
            bytes = names.iter().fold(bytes, |n, name| n.saturating_add(name.len()));
        }
    }
    if resets == 0 {
        return Ok(());
    }
    let count = fields.len().max(states.len());
    let (mut calculations, mut references, mut source_bytes) = (0usize, 0usize, 0usize);
    for field in fields {
        use pdfcraft_forms::af::Calculate;
        if field.actions.calculate != Calculate::None || field.actions.scripts.calculate.is_some() {
            calculations = calculations.saturating_add(1);
        }
        references = references.saturating_add(match &field.actions.calculate {
            Calculate::Simple { fields, .. } => fields.len(),
            Calculate::Notation(source) | Calculate::Custom(source) => source.len(),
            Calculate::None => 0,
        });
        source_bytes = source_bytes.saturating_add(field.actions.scripts.calculate.as_ref().map_or(0, String::len));
    }
    let name_bytes =
        fields.iter().fold(0usize, |n, f| n.saturating_add(f.name.len())).max(states.iter().fold(0usize, |n, f| n.saturating_add(f.name.len())));
    let widgets = fields.iter().fold(0usize, |n, f| n.saturating_add(f.widgets.len()));
    let per_pass = count
        .saturating_mul(1usize.saturating_add(calculations).saturating_add(references))
        .saturating_add(source_bytes)
        .saturating_add(name_bytes)
        .saturating_add(widgets);
    // Bound the expansion scan before doing it, then charge only the actual selections
    // for the forms layer's membership and validation scans in both reset passes.
    let comparisons = count.saturating_mul(selectors).saturating_add(count.saturating_mul(bytes));
    let mut work = resets.saturating_mul(per_pass).saturating_add(comparisons).saturating_add(bytes);
    if work > MAX_SCRIPT_RESET_WORK / 2 {
        return Err(invalid());
    }
    for request in requests {
        let Request::Reset(names) = request else { continue };
        if names.is_empty() {
            continue;
        }
        for field in states.iter().filter(|f| reset_name_matches(&f.name, names)) {
            work = work.saturating_add(count.saturating_mul(field.name.len().saturating_add(1)).saturating_mul(2));
            if work > MAX_SCRIPT_RESET_WORK / 2 {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// Match an exact reset selector or one of its dotted descendants.
fn reset_name_matches(name: &str, selectors: &[String]) -> bool {
    selectors.iter().any(|selector| name == selector || name.strip_prefix(selector).is_some_and(|rest| rest.starts_with('.')))
}

/// A field gaining or losing focus ([`Session::field_focus`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldFocus {
    /// The field gained focus: its `enter` scripts run.
    Enter,
    /// The field lost focus: its `exit` scripts run.
    Exit,
}

/// What scripts produced for the viewer since it last looked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsOutput {
    /// The messages scripts showed, in order, with their titles, icons and buttons.
    pub alerts: Vec<Message>,
    pub console: Vec<String>,
    pub requests: Vec<Request>,
    /// Script errors (as Acrobat's console shows them).
    pub errors: Vec<String>,
    /// Which of `errors` are document scripts' (#3940), told apart by where they came from and
    /// not by their text, which a field script's error can share: each is reported once, however
    /// often the document scripts run again.
    pub doc_errors: Vec<String>,
}

/// Console lines one output keeps, from every script an edit runs together (a form's
/// calculations, say): each run's own cap ([`pdfcraft_js::xfa::MAX_CONSOLE`]) would otherwise
/// be multiplied by the number of fields a document declares.
pub const MAX_CONSOLE_LINES: usize = 10_000;
/// Bytes of console text one output keeps, for the same reason.
pub const MAX_CONSOLE_BYTES: usize = 1 << 20;
/// Alerts one output keeps.
pub const MAX_ALERTS: usize = 1_000;

impl JsOutput {
    /// Whether scripts produced nothing: no alerts, console lines, requests or errors.
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty() && self.console.is_empty() && self.requests.is_empty() && self.errors.is_empty()
    }

    /// Adds `lines` to the console within [`MAX_CONSOLE_LINES`] and [`MAX_CONSOLE_BYTES`]; what
    /// is past them is left out, which one closing line says.
    fn say(&mut self, lines: impl IntoIterator<Item = String>) {
        let note = format!("more console lines were left out: the scripts printed more than {MAX_CONSOLE_LINES} lines or {MAX_CONSOLE_BYTES} bytes");
        // Once the notice is written the console is closed: nothing may follow it, and a later
        // rejection must not add a second copy.
        if self.console.last() == Some(&note) {
            return;
        }
        let mut bytes: usize = self.console.iter().map(String::len).sum();
        let mut dropped = false;
        for l in lines {
            if self.console.len() >= MAX_CONSOLE_LINES || bytes.saturating_add(l.len()) > MAX_CONSOLE_BYTES {
                dropped = true;
                break;
            }
            bytes += l.len();
            self.console.push(l);
        }
        if dropped && !self.console.iter().any(|l| l == &note) {
            self.console.push(note);
        }
    }

    /// Adds `alerts` within [`MAX_ALERTS`].
    fn alert(&mut self, alerts: impl IntoIterator<Item = Message>) {
        let room = MAX_ALERTS.saturating_sub(self.alerts.len());
        self.alerts.extend(alerts.into_iter().take(room));
    }

    /// Adds what a field's own script produced: its alerts, console lines, errors and requests,
    /// except a request to print, show a side panel, open Save As, or open or save an attached
    /// file, which is noted on the console and dropped.
    fn absorb(&mut self, o: &Outcome) {
        self.alert(o.alerts.iter().cloned());
        self.say(o.console.iter().cloned());
        // A field's own scripts (keystroke, format, validate, calculate) run unasked: only a
        // button's or the console's may print, show or hide a side panel, open Save As, or open
        // or save an attached file (#4088).
        // The warnings go through `say` like any other console line, so they count against the
        // line and byte caps instead of growing the console past them.
        let mut notes = Vec::new();
        for r in &o.requests {
            match r {
                Request::Print => notes
                    .push("Print requested by an automatic field script was ignored; use a button or the JavaScript console to print".to_string()),
                Request::ShowPanel(_) => notes.push("A field script asked to show a side panel; only a button may show a side panel".to_string()),
                Request::SaveAs => notes.push("A field script asked to save the document as a new file; only a button may open Save As".to_string()),
                Request::OpenAttachment { name, .. } => {
                    notes.push(format!("A field script asked to open the attached file {name}; only a button may open an attached file"))
                }
                r => self.requests.push(r.clone()),
            }
        }
        self.say(notes);
        self.errors.extend(o.error.iter().cloned());
        // The document scripts run before every field script, so a failing one would repeat
        // once per calculation; it is reported once.
        for e in &o.doc_errors {
            if !self.doc_errors.contains(e) {
                self.doc_errors.push(e.clone());
                self.errors.push(e.clone());
            }
        }
    }

    pub(crate) fn append(&mut self, other: JsOutput) {
        self.alert(other.alerts);
        self.say(other.console);
        self.requests.extend(other.requests);
        // As in `absorb`: a document script's error, already reported, is not repeated; a field
        // script's errors are each their own, even one worded like a document script's.
        let mut docs = other.doc_errors;
        for e in other.errors {
            if let Some(k) = docs.iter().position(|d| *d == e) {
                docs.swap_remove(k);
                if self.doc_errors.contains(&e) {
                    continue;
                }
                self.doc_errors.push(e.clone());
            }
            self.errors.push(e);
        }
    }

    /// Takes out the alerts, console lines, requests and errors. The record of the document
    /// scripts' errors stays, so they are still reported once.
    fn set_aside(&mut self) -> JsOutput {
        JsOutput {
            alerts: std::mem::take(&mut self.alerts),
            console: std::mem::take(&mut self.console),
            requests: std::mem::take(&mut self.requests),
            errors: std::mem::take(&mut self.errors),
            doc_errors: Vec::new(),
        }
    }

    /// Puts back what [`JsOutput::set_aside`] took out and returns what was added since.
    fn put_back(&mut self, waiting: JsOutput) -> JsOutput {
        let own = self.set_aside();
        self.alerts = waiting.alerts;
        self.console = waiting.console;
        self.requests = waiting.requests;
        self.errors = waiting.errors;
        own
    }
}

impl Session {
    /// Runs `f` and returns its value with the script output `f` added to document `id`. The
    /// output limits count that output alone, and the output that was waiting stays for
    /// [`Session::take_js_output`]. When `f` fails, its output joins the waiting output.
    pub fn with_own_js_output<T, E>(&mut self, id: DocId, f: impl FnOnce(&mut Session) -> Result<T, E>) -> Result<(T, JsOutput), E> {
        let waiting = self.docs.iter_mut().find(|d| d.id == id).map(|d| d.js_output.set_aside());
        let result = f(self);
        let mut doc = self.docs.iter_mut().find(|d| d.id == id);
        let own = doc.as_mut().zip(waiting).map(|(d, w)| d.js_output.put_back(w)).unwrap_or_default();
        match result {
            Ok(value) => Ok((value, own)),
            Err(e) => {
                if let Some(d) = doc {
                    d.js_output.append(own);
                }
                Err(e)
            }
        }
    }
}

fn kind(k: FieldKind) -> pdfcraft_js::FieldType {
    use pdfcraft_js::FieldType as T;
    match k {
        FieldKind::Text => T::Text,
        FieldKind::CheckBox => T::CheckBox,
        FieldKind::Radio => T::RadioButton,
        FieldKind::PushButton => T::Button,
        FieldKind::Combo => T::ComboBox,
        FieldKind::List => T::ListBox,
        FieldKind::Signature => T::Signature,
    }
}

/// `field.display` from the first widget's annotation flags.
fn display(doc: &pdfcraft_cos::Document, f: &Field) -> i32 {
    let flags = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().and_then(|d| d.int(b"F"))).unwrap_or(4);
    if flags & 2 != 0 {
        pdfcraft_js::DISPLAY_HIDDEN
    } else if flags & 32 != 0 {
        pdfcraft_js::DISPLAY_NO_VIEW
    } else if flags & 4 == 0 {
        pdfcraft_js::DISPLAY_NO_PRINT
    } else {
        pdfcraft_js::DISPLAY_VISIBLE
    }
}

/// Document-local PDF field identity used by snapshots, tools and native hit tests.
pub fn field_object_id(object: pdfcraft_cos::ObjRef) -> u64 {
    (u64::from(object.num) << 16) | u64::from(object.generation)
}

/// Check that a supplied field identity belongs to the requested field name.
fn validate_script_target(target: Option<&str>, identity: Option<u64>, fields: &[Field]) -> Result<(), EditError> {
    if let Some(id) = identity
        && !fields.iter().any(|f| Some(f.name.as_str()) == target && field_object_id(f.obj) == id)
    {
        return Err(EditError::Invalid("script field identity does not match the named field in this document".into()));
    }
    Ok(())
}

/// Snapshot a current form field using page geometry already read once for this runner.
/// Values/appearances come from `doc` and `f`, never from the cached calculation plan.
pub fn field_state(doc: &pdfcraft_cos::Document, f: &Field, geometry: &pdfcraft_js::DocInfo) -> pdfcraft_js::FieldState {
    let mut s = pdfcraft_js::FieldState::new(f.name.clone(), kind(f.kind), f.value.clone());
    s.object_id = Some(field_object_id(f.obj));
    s.default = f.default.clone();
    s.default_known = pdfcraft_forms::default_value_known(doc, f);
    s.readonly = f.read_only();
    s.required = f.has(pdfcraft_forms::flags::REQUIRED);
    s.display = display(doc, f);
    let look = pdfcraft_forms::look(doc, f);
    s.text_color = Some(std::iter::once("RGB".to_string()).chain(look.text.map(|v| v.to_string())).collect());
    s.char_limit = f.max_len;
    s.options = match f.kind {
        FieldKind::CheckBox | FieldKind::Radio => f.widgets.iter().filter_map(|w| w.on_state.clone()).map(|o| (o.clone(), o)).collect(),
        _ => f.options.clone(),
    };
    s.caption = f.widgets.first().and_then(|w| w.caption.clone());
    s.caption_mixed = f.widgets.iter().any(|w| w.caption != s.caption);
    s.line_width = look.width;
    s.border_style = match look.style {
        pdfcraft_forms::BorderStyle::Solid => "solid",
        pdfcraft_forms::BorderStyle::Dashed => "dashed",
        pdfcraft_forms::BorderStyle::Beveled => "beveled",
        pdfcraft_forms::BorderStyle::Inset => "inset",
        pdfcraft_forms::BorderStyle::Underline => "underline",
    }
    .into();
    s.stroke_color = Some(
        look.border
            .map_or_else(|| std::sync::Arc::from(["T".into()]), |rgb| std::iter::once("RGB".into()).chain(rgb.map(|v| v.to_string())).collect()),
    );
    s.fill_color = Some(
        look.fill.map_or_else(|| std::sync::Arc::from(["T".into()]), |rgb| std::iter::once("RGB".into()).chain(rgb.map(|v| v.to_string())).collect()),
    );
    s.user_name = doc
        .get(f.obj)
        .as_dict()
        .and_then(|d| d.get(b"TU").map(|v| doc.resolve(v)))
        .and_then(|v| v.as_string().map(|v| v.to_text()))
        .unwrap_or_default();
    s.text_size =
        f.da.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .find_map(|p| (p.get(1) == Some(&"Tf")).then(|| p.first()?.parse::<f64>().ok()).flatten())
            .unwrap_or(0.0);
    if let Some(w) = f.widgets.first() {
        let page = w.page.unwrap_or(0);
        s.page_rotation = geometry.page_rotations.get(page).copied().unwrap_or(0);
        s.page_crop = geometry.page_boxes.get(page).map_or([0.0; 4], |boxes| boxes[1]);
        s.rect = Some(pdfcraft_js::rotate_rect(w.rect, s.page_crop, s.page_rotation));
        s.icon = doc
            .get(w.obj)
            .as_dict()
            .and_then(|d| d.get(b"MK").map(|v| doc.resolve(v)))
            .and_then(|v| v.as_dict().and_then(|d| d.get(b"I")).and_then(pdfcraft_cos::Object::as_ref))
            .map(|r| ((r.num as u64) << 16) | r.generation as u64);
    }

    s.icon_mixed = f.widgets.iter().any(|w| {
        let icon = doc
            .get(w.obj)
            .as_dict()
            .and_then(|d| d.get(b"MK").map(|v| doc.resolve(v)))
            .and_then(|v| v.as_dict().and_then(|d| d.get(b"I")).and_then(pdfcraft_cos::Object::as_ref))
            .map(field_object_id);
        icon != s.icon
    });
    s
}

/// The changes a script made, against the fields as they were.
fn changes(
    before: &[pdfcraft_js::FieldState],
    after: &[pdfcraft_js::FieldState],
    formatted: &std::collections::BTreeSet<String>,
) -> Vec<FieldChange> {
    if after.is_empty() {
        return Vec::new();
    }
    // Outcomes contain only fields touched by the script. Index the original snapshot once,
    // keeping the first match as the old linear lookup did for malformed duplicate names.
    let mut by_name = std::collections::HashMap::with_capacity(before.len());
    for field in before {
        by_name.entry(field.name.as_str()).or_insert(field);
    }
    let by_id: std::collections::HashMap<_, _> = before.iter().filter_map(|f| f.object_id.map(|id| (id, f))).collect();
    let mut names = std::collections::HashSet::new();
    let duplicates: std::collections::HashSet<_> =
        before.iter().filter_map(|f| (!names.insert(f.name.as_str())).then_some(f.name.as_str())).collect();
    after
        .iter()
        .filter_map(|a| {
            let b = if let Some(id) = a.object_id { by_id.get(&id)? } else { by_name.get(a.name.as_str())? };
            let c = FieldChange {
                name: a.name.clone(),
                object: a
                    .object_id
                    .filter(|_| duplicates.contains(a.name.as_str()))
                    .map(|id| pdfcraft_cos::ObjRef::new((id >> 16) as u32, id as u16)),
                value: (a.value != b.value).then(|| a.value.clone()),
                formatted: a.value != b.value && formatted.contains(&a.name),
                read_only: (a.readonly != b.readonly).then_some(a.readonly),
                required: (a.required != b.required).then_some(a.required),
                display: (a.display != b.display).then_some(a.display),
                rect: (a.rect != b.rect).then(|| a.rect.map(|r| pdfcraft_js::unrotate_rect(r, b.page_crop, b.page_rotation))).flatten(),
                stroke_color: a.stroke_color.as_deref().filter(|_| a.stroke_color != b.stroke_color).map(<[_]>::to_vec),
                text_size: (a.text_size != b.text_size).then_some(a.text_size),
                line_width: (a.line_width != b.line_width).then_some(a.line_width),
                border_style: (a.border_style != b.border_style).then(|| a.border_style.clone()),
                user_name: (a.user_name != b.user_name).then(|| a.user_name.clone()),
                icon: (a.icon != b.icon || (a.icon_written && b.icon_mixed)).then_some(a.icon),
                text_color: a.text_color.as_deref().filter(|_| a.text_color != b.text_color).map(<[_]>::to_vec),
                fill_color: a.fill_color.as_deref().filter(|_| a.fill_color != b.fill_color).map(<[_]>::to_vec),
                caption: (a.caption != b.caption || (a.caption_written && b.caption_mixed)).then(|| a.caption.clone()).flatten(),
            };
            (c.value.is_some()
                || c.read_only.is_some()
                || c.required.is_some()
                || c.display.is_some()
                || c.caption.is_some()
                || c.text_color.is_some()
                || c.fill_color.is_some()
                || c.rect.is_some()
                || c.stroke_color.is_some()
                || c.text_size.is_some()
                || c.line_width.is_some()
                || c.border_style.is_some()
                || c.user_name.is_some()
                || c.icon.is_some())
            .then_some(c)
        })
        .collect()
}

/// Runs field scripts with the document's object model and keeps what they print or ask for.
pub type Runtime = std::sync::Arc<std::sync::Mutex<pdfcraft_js::session::ScriptSession>>;

/// Invalidate pending results while retaining the document's native worker reservation.
pub(crate) fn replace_runtime(runtime: &mut Runtime) {
    let Ok(mut old) = runtime.lock() else { return };
    let next = old.fresh_realm();
    drop(old);
    *runtime = std::sync::Arc::new(std::sync::Mutex::new(next));
}

pub struct JsRunner {
    pub runtime: Runtime,
    pub doc: pdfcraft_js::DocInfo,
    pub doc_scripts: Vec<String>,
    pub output: JsOutput,
    pub(crate) refused: Option<String>,
    pub(crate) callbacks_touched_realm: bool,
    #[cfg(not(target_arch = "wasm32"))]
    callback_started: Option<std::time::Instant>,
    /// Source/order cache for this edit only; each callback still snapshots current field values.
    calculations: Option<Vec<(usize, String)>>,
    // Cache at most 4,096 field identities in the current edit. Entries only survive value-only
    // callbacks. Appearance and structural edits clear them before the next snapshot.
    snapshot_fields: std::collections::HashMap<pdfcraft_cos::ObjRef, pdfcraft_js::FieldState>,
    // The last callback's snapshot. A calculation chain changes one or two values per callback,
    // so the next callback patches those entries instead of rebuilding every field.
    last_snapshot: Option<std::sync::Arc<Vec<pdfcraft_js::FieldState>>>,
}

impl JsRunner {
    pub(crate) fn invalidate_field_snapshots(&mut self) {
        self.snapshot_fields.clear();
        self.last_snapshot = None;
    }

    fn callback_fields(&mut self, doc: &pdfcraft_cos::Document, fields: &[Field]) -> std::sync::Arc<Vec<pdfcraft_js::FieldState>> {
        if let Some(snapshot) = self.last_snapshot.as_mut()
            && snapshot.len() == fields.len()
            && snapshot.iter().zip(fields).all(|(state, field)| state.object_id == Some(field_object_id(field.obj)))
        {
            let stale = |state: &pdfcraft_js::FieldState, field: &Field| {
                state.name != field.name
                    || state.value != field.value
                    || state.readonly != field.read_only()
                    || state.required != field.has(pdfcraft_forms::flags::REQUIRED)
            };
            if snapshot.iter().zip(fields).any(|(state, field)| stale(state, field)) {
                for (state, field) in std::sync::Arc::make_mut(snapshot).iter_mut().zip(fields) {
                    if stale(state, field) {
                        state.name.clone_from(&field.name);
                        state.value.clone_from(&field.value);
                        state.readonly = field.read_only();
                        state.required = field.has(pdfcraft_forms::flags::REQUIRED);
                    }
                }
            }
            if let Ok(mut runtime) = self.runtime.lock() {
                runtime.stamp_fields(snapshot);
            }
            return std::sync::Arc::clone(snapshot);
        }
        let states = fields
            .iter()
            .map(|field| {
                if self.snapshot_fields.len() >= 4096 && !self.snapshot_fields.contains_key(&field.obj) {
                    return field_state(doc, field, &self.doc);
                }
                let template = self.snapshot_fields.entry(field.obj).or_insert_with(|| {
                    let mut state = field_state(doc, field, &self.doc);
                    state.value.clear();
                    state
                });
                let mut state = template.clone();
                state.name.clone_from(&field.name);
                state.value.clone_from(&field.value);
                state.readonly = field.read_only();
                state.required = field.has(pdfcraft_forms::flags::REQUIRED);
                state
            })
            .collect();
        let mut states = std::sync::Arc::new(states);
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.stamp_fields(&mut states);
        }
        self.last_snapshot = Some(std::sync::Arc::clone(&states));
        states
    }

    /// Forget the per-edit calculation plan after a calculation source is changed.
    pub(crate) fn invalidate_calculations(&mut self) {
        self.calculations = None;
    }

    /// Refresh structural/document sources between batch children, retaining the realm,
    /// accumulated output and the current viewer snapshot. Ordinary value edits reuse caches.
    pub(crate) fn refresh_after_batch_edit(&mut self, cos: &pdfcraft_cos::Document, edit: &Edit) {
        if matches!(edit, Edit::SetFieldValue { .. } | Edit::ResetForm { .. } | Edit::ApplyScriptChanges { .. } | Edit::Batch { .. }) {
            return;
        }
        let mut current = Self::new(cos, &self.doc.file_name, self.doc.runtime_highlight);
        current.doc.zoom = self.doc.zoom;
        current.doc.page = self.doc.page;
        self.doc = current.doc;
        self.doc_scripts = current.doc_scripts;
        self.calculations = None;
        self.invalidate_field_snapshots();
    }

    /// A runner for `cos`: what scripts read about the document (pages, info, whether it is
    /// an XFA form) and its document-level scripts, read once. `highlight`: whether the viewer
    /// highlights form fields (`app.runtimeHighlight`).
    pub fn new(cos: &pdfcraft_cos::Document, file_name: &str, highlight: bool) -> JsRunner {
        let xfa = pdfcraft_xfa::xfa_shape(cos).is_some();
        #[cfg(test)]
        SNAPSHOT_WORK.with(|c| {
            let (g, p) = c.get();
            c.set((g + 1, p));
        });
        let pages = pdfcraft_model::pages(cos);
        let doc = pdfcraft_js::DocInfo {
            file_name: file_name.to_string(),
            num_pages: pages.len(),
            page: 0,
            info: info(cos),
            page_boxes: pdfcraft_organize::page_boxes(cos).unwrap_or_default(),
            page_rotations: pages.iter().map(|page| page.rotation(cos)).collect(),
            xfa,
            zoom: 100.0,
            runtime_highlight: highlight,
            icon_epoch: 0,
            icon_versions: Default::default(),
        };
        // Once XFA owns the form, the PDF container's scripts belong to its fallback view.
        // Keep them in the file and in Document JavaScripts, but do not initialize an
        // AcroForm context with them: a viewer-compatibility alert would be misleading.
        // Packets alone are not enough: static XFA and an unlaid-out shell keep their scripts.
        let xfa_active = xfa && pdfcraft_xfa::existing_layout(cos).is_some();
        let doc_scripts = if xfa_active { Vec::new() } else { pdfcraft_forms::document_scripts(cos) };
        JsRunner {
            doc,
            doc_scripts,
            output: JsOutput::default(),
            refused: None,
            callbacks_touched_realm: false,
            #[cfg(not(target_arch = "wasm32"))]
            callback_started: None,
            runtime: Default::default(),
            calculations: None,
            snapshot_fields: Default::default(),
            last_snapshot: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn callback_time_left(&mut self) -> std::time::Duration {
        std::time::Duration::from_secs(10).saturating_sub(self.callback_started.get_or_insert_with(std::time::Instant::now).elapsed())
    }
}

/// Index calculation sources by PDF object identity, retaining their matching snapshot positions.
fn calculation_scripts(cos: &pdfcraft_cos::Document, fields: &[Field]) -> Vec<(usize, String)> {
    #[cfg(test)]
    SNAPSHOT_WORK.with(|c| {
        let (g, p) = c.get();
        c.set((g, p + 1));
    });
    let by_object: std::collections::HashMap<_, _> = fields.iter().enumerate().map(|(i, f)| (f.obj, (i, f))).collect();
    pdfcraft_forms::calculation_order(cos, fields)
        .fields
        .iter()
        .filter_map(|ordered| {
            let (index, field) = by_object.get(&ordered.obj)?;
            let script = field.actions.scripts.calculate.clone().or_else(|| {
                let source = pdfcraft_forms::af::calculate_js(&field.actions.calculate)?;
                Some(match &field.actions.calculate {
                    pdfcraft_forms::af::Calculate::Simple { .. } => format!("event.value = {source}"),
                    _ => source,
                })
            })?;
            Some((*index, script))
        })
        .collect()
}

/// The field highlight the last of `requests` that sets it asks for (`app.runtimeHighlight`).
fn last_highlight(requests: &[Request]) -> Option<bool> {
    requests.iter().rev().find_map(|r| match r {
        Request::Highlight(on) => Some(*on),
        _ => None,
    })
}

/// The Info dictionary as `this.info` keys (lower-case).
fn info(cos: &pdfcraft_cos::Document) -> Vec<(String, String)> {
    let Some(d) = cos.trailer().get(b"Info").map(|i| cos.resolve(i)).and_then(|i| i.as_dict().cloned()) else { return Vec::new() };
    d.iter()
        .filter_map(|(k, v)| match &*cos.resolve(v) {
            pdfcraft_cos::Object::String(s) => Some((String::from_utf8_lossy(k).to_lowercase(), s.to_text())),
            _ => None,
        })
        .collect()
}

impl Scripts for JsRunner {
    fn stopped(&self) -> bool {
        self.refused.is_some()
    }

    /// Run field `target`'s script for `event` over the form's fields, keeping what it showed and
    /// asked for; a refusal carries its last message.
    fn run(&mut self, event: FieldEvent, script: &str, target: &Field, value: &str, fields: &[Field], doc: &pdfcraft_cos::Document) -> ScriptResult {
        let unchanged = || ScriptResult { rc: true, value: value.to_string(), value_set: false, changes: Vec::new(), message: None };
        if self.stopped() {
            return unchanged();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.callback_time_left().is_zero() {
            self.refused = Some("form script callbacks exceeded their shared execution budget".into());
            return unchanged();
        }
        let states = self.callback_fields(doc, fields);
        let mut ev = pdfcraft_js::Event::field(event.name(), &target.name, value);
        ev.target_id = Some((u64::from(target.obj.num) << 16) | u64::from(target.obj.generation));
        let calculations = self.calculations.get_or_insert_with(|| calculation_scripts(doc, fields)).clone();
        let job = pdfcraft_js::session::Job {
            script: script.into(),
            event: ev,
            doc: self.doc.clone(),
            fields: states,
            doc_scripts: self.doc_scripts.clone(),
            calculations,
        };
        #[cfg(not(target_arch = "wasm32"))]
        let remaining = self.callback_time_left();
        let o = match self.runtime.lock() {
            #[cfg(not(target_arch = "wasm32"))]
            Ok(mut runtime) if !remaining.is_zero() => runtime.run_with_limit(job, &[], remaining),
            #[cfg(not(target_arch = "wasm32"))]
            Ok(_) => Outcome {
                execution_refused: true,
                realm_untouched: true,
                error: Some("form script callbacks exceeded their shared execution budget".into()),
                ..Default::default()
            },
            #[cfg(target_arch = "wasm32")]
            Ok(mut runtime) => runtime.run(job, &[]),
            Err(_) => Outcome { execution_refused: true, error: Some("document script runtime is unavailable".into()), ..Default::default() },
        };
        self.callbacks_touched_realm |= !o.realm_untouched;
        self.output.absorb(&o);
        if o.execution_refused || o.reset_limit_exceeded || o.calculation_limit_exceeded {
            self.refused = Some(o.error.clone().unwrap_or_else(|| "form script exceeded its execution budget".into()));
        }
        if o.error.is_some() || o.reset_limit_exceeded || o.calculation_limit_exceeded {
            // As in Acrobat, a failing script leaves the value alone (the error goes to the console).
            return unchanged();
        }
        // The edit's later scripts read the field highlight this one set.
        if let Some(on) = last_highlight(&o.requests) {
            self.doc.runtime_highlight = on;
        }
        // Most field callbacks only set event.value. Hand the unique snapshot to the
        // session so stamping field identities does not clone every field. Reconstruct
        // the baseline only for a callback that also wrote to Field objects.
        let changes = if o.changed.is_empty() {
            Vec::new()
        } else {
            let baseline = self.callback_fields(doc, fields);
            changes(&baseline, &o.changed, &o.formatted)
        };
        if changes.iter().any(|change| {
            *change != FieldChange { name: change.name.clone(), object: change.object, value: change.value.clone(), ..Default::default() }
        }) {
            self.invalidate_field_snapshots();
        }
        ScriptResult {
            rc: o.rc,
            value: o.value.clone(),
            value_set: o.value_set,
            changes,
            message: (!o.rc).then(|| o.alerts.last().map(|m| m.text.clone())).flatten(),
        }
    }

    /// Retry appearance formatting in isolation so the font fallback cannot replay persistent side effects.
    fn rerun_with_english_dates(
        &mut self,
        event: FieldEvent,
        script: &str,
        target: &Field,
        value: &str,
        fields: &[Field],
        doc: &pdfcraft_cos::Document,
    ) -> Option<ScriptResult> {
        if self.stopped() {
            return None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.callback_time_left().is_zero() {
            self.refused = Some("form script callbacks exceeded their shared execution budget".into());
            return None;
        }
        let states = self.callback_fields(doc, fields);
        let mut ev = pdfcraft_js::Event::field(event.name(), &target.name, value);
        ev.target_id = Some(field_object_id(target.obj));
        let calculations = self.calculations.get_or_insert_with(|| calculation_scripts(doc, fields));
        #[cfg(target_arch = "wasm32")]
        let o = pdfcraft_js::with_english_printd(true, || {
            pdfcraft_js::run_shared_calculations(script, &ev, &self.doc, states, &self.doc_scripts, calculations, Limits::default())
        });
        #[cfg(not(target_arch = "wasm32"))]
        let o = {
            let job = pdfcraft_js::session::Job {
                script: script.into(),
                event: ev,
                doc: self.doc.clone(),
                fields: states,
                doc_scripts: self.doc_scripts.clone(),
                calculations: calculations.clone(),
            };
            let remaining = self.callback_time_left();
            let outcome = if remaining.is_zero() {
                Outcome { execution_refused: true, ..Default::default() }
            } else if let Ok(mut runtime) = self.runtime.lock() {
                runtime.run_english_retry(job, remaining)
            } else {
                Outcome { execution_refused: true, error: Some("document script runtime is unavailable".into()), ..Default::default() }
            };
            if outcome.execution_refused {
                self.refused = Some(outcome.error.unwrap_or_else(|| "form script callbacks exceeded their shared execution budget".into()));
                return None;
            }
            outcome
        };
        if o.error.is_some() || o.reset_limit_exceeded || o.calculation_limit_exceeded {
            return None;
        }
        Some(ScriptResult { rc: o.rc, value: o.value.clone(), value_set: o.value_set, changes: Vec::new(), message: None })
    }
}

/// A document-level JavaScript: its name and source.
pub fn document_scripts(cos: &pdfcraft_cos::Document) -> Vec<(String, String)> {
    pdfcraft_forms::document_scripts_named(cos)
}

impl crate::Document {
    /// Field Properties ▸ Actions: field `name`'s action per trigger.
    pub fn field_actions(&self, name: &str) -> Vec<(pdfcraft_forms::Trigger, pdfcraft_forms::FieldAction)> {
        self.editor.as_ref().and_then(|e| pdfcraft_forms::field_actions(&e.cos, name).ok()).unwrap_or_default()
    }

    /// Field Properties ▸ Actions: how many actions are chained after each trigger's first one.
    pub fn field_action_chains(&self, name: &str) -> Vec<(pdfcraft_forms::Trigger, usize)> {
        self.editor.as_ref().and_then(|e| pdfcraft_forms::field_action_chains(&e.cos, name).ok()).unwrap_or_default()
    }

    /// Document JavaScripts (name, source), in name order.
    pub fn document_scripts(&self) -> Vec<(String, String)> {
        self.editor.as_ref().map(|e| document_scripts(&e.cos)).unwrap_or_default()
    }
}

/// When a signature field's XFA signing scripts run ([`Session::run_sign_event`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignEvent {
    /// Just before the field is signed (`preSign`): what it changes is signed too.
    Before,
    /// Once the field is signed (`postSign`).
    After,
}

impl SignEvent {
    /// The template's name for the event.
    pub fn activity(self) -> &'static str {
        match self {
            SignEvent::Before => "preSign",
            SignEvent::After => "postSign",
        }
    }
}

/// A PDF document action dispatched by the host through its existing AcroForm session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentScriptEvent {
    CatalogOpen,
    WillSave,
    WillPrint,
}

impl DocumentScriptEvent {
    fn name(self) -> &'static str {
        match self {
            Self::CatalogOpen => "Open",
            Self::WillSave => "WillSave",
            Self::WillPrint => "WillPrint",
        }
    }
}

impl Session {
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript (on by default).
    pub fn set_javascript(&mut self, on: bool) {
        self.js_off = !on;
        if !on {
            for doc in &mut self.docs {
                replace_runtime(&mut doc.js_runtime);
            }
        }
    }

    /// Whether JavaScript is on (Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript).
    pub fn javascript(&self) -> bool {
        !self.js_off
    }

    /// Whether the viewer highlights form fields (Highlight fields), which forms' scripts read as
    /// `app.runtimeHighlight`. A script that changes it asks for [`Request::Highlight`]; the
    /// viewer then calls this with the new setting.
    pub fn set_field_highlight(&mut self, on: bool) {
        self.field_highlight = on;
    }

    /// What scripts printed or asked for since the last call (alerts, console, print, …).
    pub fn take_js_output(&mut self, id: DocId) -> JsOutput {
        self.docs.iter_mut().find(|d| d.id == id).map(|d| std::mem::take(&mut d.js_output)).unwrap_or_default()
    }

    /// The last field-highlight change (`app.runtimeHighlight`) scripts asked for in document
    /// `id`, taken out of its waiting output; the rest stays for [`Session::take_js_output`].
    pub fn take_js_highlight(&mut self, id: DocId) -> Option<bool> {
        let doc = self.docs.iter_mut().find(|d| d.id == id)?;
        let mut last = None;
        doc.js_output.requests.retain(|r| match r {
            Request::Highlight(on) => {
                last = Some(*on);
                false
            }
            _ => true,
        });
        last
    }

    /// Widget `widget` of field `name` in document `id` gained or lost focus: on a laid-out XFA form,
    /// run the field's `enter` or `exit` scripts, as one undoable edit when they change something
    /// (and none when they don't). An exit that the field's new value already ran (the value's
    /// change runs it) isn't run a second time. Returns whether the document changed; what the
    /// scripts showed or asked for is in [`Session::take_js_output`].
    pub fn field_focus(&mut self, id: DocId, name: &str, widget: usize, focus: FieldFocus) -> Result<bool, EditError> {
        if self.js_off {
            return Ok(false);
        }
        let doc = self.doc_mut(id)?;
        let Some(tpl) = doc.xfa_template.clone().filter(|t| pdfcraft_xfa::has_focus_scripts(t)) else { return Ok(false) };
        let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return Ok(false) };
        let Some(som) = doc.form.iter().find(|f| f.name == name).and_then(|f| crate::xfa::widget_som(cos, f, widget)) else { return Ok(false) };
        let activity = match focus {
            FieldFocus::Enter => "enter",
            FieldFocus::Exit => "exit",
        };
        if !crate::xfa::has_event(cos, &tpl, &som, activity) {
            doc.xfa_exited.remove(&som);
            return Ok(false);
        }
        // Gaining focus starts afresh; losing it runs the exit unless the change just did.
        if doc.xfa_exited.remove(&som) && focus == FieldFocus::Exit {
            return Ok(false);
        }
        let before = doc.generation;
        self.apply(id, Edit::XfaEvent { som, activity: activity.into() })?;
        Ok(self.get(id).is_some_and(|d| d.generation != before))
    }

    /// Run signature field `field`'s XFA signing scripts for `event` in document `id`: `preSign`
    /// before it is signed, `postSign` after, as one undoable step, as a form's signing events run
    /// (DA 31's dates the signature and locks the date). `Ok(false)`, with nothing changed, when
    /// there are none: not a laid-out XFA form, JavaScript off, no such field, or no such script.
    /// What the scripts said or asked for is in [`Session::take_js_output`].
    pub fn run_sign_event(&mut self, id: DocId, field: &str, event: SignEvent) -> Result<bool, EditError> {
        if self.get(id).is_none() {
            return Err(EditError::NoDocument);
        }
        let Some(som) = self.sign_event_som(id, field, event) else { return Ok(false) };
        self.apply(id, Edit::XfaEvent { som, activity: event.activity().into() })?;
        Ok(true)
    }

    /// Whether signature field `field` of document `id` has XFA signing scripts for `event` that
    /// [`Session::run_sign_event`] would run, without running them.
    pub fn sign_event_scripted(&self, id: DocId, field: &str, event: SignEvent) -> bool {
        self.sign_event_som(id, field, event).is_some()
    }

    /// The SOM path of signature field `field` when it has scripts for `event` to run.
    fn sign_event_som(&self, id: DocId, field: &str, event: SignEvent) -> Option<String> {
        if self.js_off {
            return None;
        }
        let doc = self.get(id)?;
        let (tpl, editor) = (doc.xfa_template.as_ref()?, doc.editor.as_ref()?);
        let f = doc.form.iter().find(|f| f.name == field)?;
        crate::xfa::scripted_som(&editor.cos, tpl, f, event.activity())
    }

    /// Run `script` in document `id`, for a button (`target`: the field, event "Mouse Up") or
    /// the console (no target). Field changes and `resetForm` are applied as one undoable step;
    /// everything else the script asked for is returned.
    pub fn run_javascript(&mut self, id: DocId, script: &str, target: Option<&str>) -> Result<Outcome, EditError> {
        self.run_javascript_answers(id, script, target, &[])
    }

    /// Run a button/console event with ordered headless prompt answers and commit field changes as one undo step.
    pub fn run_javascript_answers(
        &mut self,
        id: DocId,
        script: &str,
        target: Option<&str>,
        answers: &[Option<String>],
    ) -> Result<Outcome, EditError> {
        self.run_javascript_answers_target(id, script, target, None, answers)
    }
    /// Run with an optional exact PDF field identity; name-only callers keep first-match behavior.
    pub fn run_javascript_target(&mut self, id: DocId, script: &str, target: Option<&str>, target_id: Option<u64>) -> Result<Outcome, EditError> {
        self.run_javascript_answers_target(id, script, target, target_id, &[])
    }
    /// Exact-target counterpart of run_javascript_answers, including deterministic prompt replies.
    pub fn run_javascript_answers_target(
        &mut self,
        id: DocId,
        script: &str,
        target: Option<&str>,
        target_id: Option<u64>,
        answers: &[Option<String>],
    ) -> Result<Outcome, EditError> {
        self.run_javascript_job(id, script, (target, target_id), answers, None, false)
    }

    pub(crate) fn run_opening_javascript(&mut self, id: DocId) -> Result<Outcome, EditError> {
        self.run_javascript_job(id, "", (None, None), &[], None, true)
    }

    /// Run an explicitly dispatched PDF document action in the document's retained runtime.
    pub fn run_document_script_event(&mut self, id: DocId, event: DocumentScriptEvent, script: &str) -> Result<Outcome, EditError> {
        self.run_javascript_job(id, script, (None, None), &[], Some(event), false)
    }

    /// Route generated XFA clicks to the XFA engine, or run a synchronous AcroForm job and apply its outcome.
    fn run_javascript_job(
        &mut self,
        id: DocId,
        script: &str,
        (target, target_id): (Option<&str>, Option<u64>),
        answers: &[Option<String>],
        document_event: Option<DocumentScriptEvent>,
        opening: bool,
    ) -> Result<Outcome, EditError> {
        if self.js_off {
            return Err(EditError::Invalid("JavaScript is turned off (Preferences ▸ JavaScript)".into()));
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = doc.editor.as_ref().map(|e| e.cos.clone()).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        validate_script_target(target, target_id, &doc.form)?;
        // A button PdfCraft generated from an XFA template runs its XFA click script.
        if let Some(t) = target
            && let Some(tpl) = doc.xfa_template.clone()
            && let Some(field) = doc.form.iter().find(|f| f.name == t && target_id.is_none_or(|id| field_object_id(f.obj) == id))
            && field.kind == crate::FormFieldKind::PushButton
            && let Some(som) = crate::xfa::clickable_som(&cos, &tpl, field)
        {
            // The click returns only the output of its own edit; output from opening or from earlier edits stays.
            let ((), out) = self.with_own_js_output(id, |s| s.apply(id, Edit::XfaEvent { som, activity: "click".into() }))?;
            return Ok(Outcome {
                rc: true,
                alerts: out.alerts,
                console: out.console,
                requests: out.requests,
                error: out.errors.first().cloned(),
                ..Default::default()
            });
        }
        let mut runner = JsRunner::new(&cos, &doc.name, self.field_highlight);
        runner.doc.zoom = doc.js_zoom;
        runner.doc.page = doc.js_page;
        let fields = pdfcraft_forms::fields(&cos);
        let states = std::sync::Arc::new(fields.iter().map(|f| field_state(&cos, f, &runner.doc)).collect::<Vec<_>>());
        let event = match target {
            Some(t) => pdfcraft_js::Event { will_commit: false, target_id, ..pdfcraft_js::Event::field("Mouse Up", t, "") },
            None => pdfcraft_js::Event::doc(document_event.map_or("Console", DocumentScriptEvent::name)),
        };
        let calculations = calculation_scripts(&cos, &fields);
        let runtime = doc.js_runtime.clone();
        let o = runtime.lock().map_err(|_| EditError::Invalid("document script runtime is unavailable".into()))?.run(
            pdfcraft_js::session::Job {
                script: script.into(),
                event,
                doc: runner.doc,
                fields: states.clone(),
                doc_scripts: runner.doc_scripts,
                calculations,
            },
            answers,
        );
        let discard = o.execution_refused && !o.realm_untouched;
        let result = self.apply_script_outcome(id, &states, o, opening);
        if discard || result.is_err() {
            self.invalidate_javascript(id);
        }
        result
    }

    /// Convert the worker snapshot delta and reset requests into permission-checked atomic document edits.
    fn apply_script_outcome(&mut self, id: DocId, states: &[pdfcraft_js::FieldState], mut o: Outcome, opening: bool) -> Result<Outcome, EditError> {
        if o.execution_refused {
            o.changed.clear();
            o.requests.clear();
            return Ok(o);
        }
        if o.calculation_limit_exceeded {
            return Err(EditError::Invalid(o.error.unwrap_or_else(|| "script calculation work limit exceeded".into())));
        }
        if o.reset_limit_exceeded {
            return Err(EditError::Invalid("script reset call limit exceeded; no script edits were committed".into()));
        }
        if o.requests.iter().any(|r| matches!(r, Request::Reset(_))) {
            let doc = self.get(id).ok_or(EditError::NoDocument)?;
            validate_reset_work(&doc.form, states, &o.requests)?;
        }
        let resets: Vec<Vec<String>> = o
            .requests
            .iter()
            .filter_map(|r| match r {
                Request::Reset(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        o.requests.retain(|r| !matches!(r, Request::Reset(_)));
        // Reset the PDF first, then apply the final live shadow. Compare against the
        // reset baseline so writes restoring an original value are not mistaken for no-ops.
        let mut baseline = std::borrow::Cow::Borrowed(states);
        let mut reset_document = if resets.is_empty() {
            None
        } else {
            let doc = self.get(id).ok_or(EditError::NoDocument)?;
            let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
            Some(editor.cos.clone())
        };
        // The calculations the script's changes set off read the field highlight it asked for.
        if let Some(on) = last_highlight(&o.requests) {
            self.field_highlight = on;
        }
        let mut edits = Vec::new();
        for names in resets {
            let names = (!names.is_empty())
                .then(|| states.iter().filter(|field| reset_name_matches(&field.name, &names)).map(|field| field.name.clone()).collect::<Vec<_>>());
            if let Some(doc) = reset_document.as_mut() {
                // The preview includes dependent calculation writes as well as direct resets.
                let (reset, recalculated) = pdfcraft_forms::reset_with_counts(doc, names.as_deref())?;
                if !opening || reset != 0 || recalculated != 0 {
                    edits.push(Edit::ResetForm { names });
                }
            }
        }
        if let Some(doc) = reset_document {
            let mut values = std::collections::HashMap::new();
            for field in pdfcraft_forms::fields(&doc) {
                values.entry((u64::from(field.obj.num) << 16) | u64::from(field.obj.generation)).or_insert(field.value);
            }
            for field in baseline.to_mut() {
                if let Some(value) = field.object_id.and_then(|id| values.get(&id)) {
                    field.value.clone_from(value);
                }
            }
        }
        let changes = changes(&baseline, &o.changed, &o.formatted);
        if !changes.is_empty() {
            edits.push(Edit::ApplyScriptChanges { changes });
        }
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: "Run JavaScript".into(), edits })?;
        }
        Ok(o)
    }
}

/// A page's words for form-field detection, in user space (underscore runs split from the
/// text around them).
fn detection_words(text: &pdfcraft_render::PageText, info: &pdfcraft_render::PageInfo) -> Vec<pdfcraft_forms::detect::Word> {
    page_words(text, info).into_iter().map(|(text, rect)| pdfcraft_forms::detect::Word { text, rect }).collect()
}

/// A page's words in reading order, in user space ([x0, y0, x1, y1]); runs of underscores are
/// words of their own.
pub(crate) fn page_words(text: &pdfcraft_render::PageText, info: &pdfcraft_render::PageInfo) -> Vec<(String, [f64; 4])> {
    words_with(text, |x, y| info.view_to_user(x, y))
        .into_iter()
        .map(|(text, r)| (text, [f64::from(r[0]), f64::from(r[1]), f64::from(r[2]), f64::from(r[3])]))
        .collect()
}

#[cfg(test)]
thread_local! {
    /// Characters read to tell underscore runs from other words.
    static UNDERSCORE_CHARS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A page's words in reading order, their boxes mapped from view space by `map` (and ordered
/// again, so that [x0, y0, x1, y1] holds whatever way `map` turns them).
pub(crate) fn words_with(text: &pdfcraft_render::PageText, map: impl Fn(f32, f32) -> [f32; 2]) -> Vec<(String, [f32; 4])> {
    let mut words: Vec<(String, [f32; 4])> = Vec::new();
    let mut last_line = u32::MAX;
    // A space glyph ends the word before it.
    let mut gap = false;
    // Whether the last word is an underscore run (a form's blank), which other glyphs don't join.
    let mut last_under = false;
    for (i, g) in text.glyphs.iter().enumerate() {
        let line = text.line_of.get(i).copied().unwrap_or(0);
        if g.text.trim().is_empty() {
            gap = true;
            continue;
        }
        let under = g.text.chars().all(|c| c == '_');
        #[cfg(test)]
        UNDERSCORE_CHARS_READ.with(|n| n.set(n.get() + g.text.len()));
        let breaks = words.is_empty() || gap || line != last_line || text.space_before.get(i).copied().unwrap_or(false) || last_under != under;
        gap = false;
        last_line = line;
        match words.last_mut() {
            Some((w, r)) if !breaks => {
                w.push_str(&g.text);
                r[0] = r[0].min(g.rect[0]);
                r[1] = r[1].min(g.rect[1]);
                r[2] = r[2].max(g.rect[2]);
                r[3] = r[3].max(g.rect[3]);
            }
            _ => {
                words.push((g.text.clone(), g.rect));
                last_under = under;
            }
        }
    }
    words
        .into_iter()
        .map(|(text, r)| {
            let a = map(r[0], r[1]);
            let b = map(r[2], r[3]);
            (text, [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])])
        })
        .collect()
}

impl Session {
    /// Prepare a form ▸ automatic field detection: the fields `pages` (0-based; empty = all)
    /// seem to ask for, named from their labels, as (page, candidate). Scanned pages (a picture
    /// and no drawn shapes) are read from their pixels, see [`crate::scan_fields`].
    pub fn detect_fields(&self, id: DocId, pages: &[usize]) -> Vec<(usize, pdfcraft_forms::detect::Candidate)> {
        self.detect_fields_turned(id, pages).into_iter().map(|(page, c, _)| (page, c)).collect()
    }

    /// [`Self::detect_fields`], with how far each field turns (`/MK /R`) to read upright.
    fn detect_fields_turned(&self, id: DocId, pages: &[usize]) -> Vec<(usize, pdfcraft_forms::detect::Candidate, i64)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return Vec::new() };
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut taken: Vec<String> = doc.form.iter().map(|f| f.name.clone()).collect();
        let all = doc.info.pages.len();
        let list: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.iter().copied().filter(|p| *p < all).collect() };
        let mut out = Vec::new();
        for page in list {
            let Some(info) = doc.info.pages.get(page) else { continue };
            let shapes = pdfcraft_forms::detect::page_shapes(cos, page);
            let existing: Vec<[f64; 4]> = doc.form.iter().flat_map(|f| f.widgets.iter()).filter(|w| w.page == Some(page)).map(|w| w.rect).collect();
            let (found, turn) = if shapes.boxes.is_empty() && shapes.rules.is_empty() && crate::scan_fields::is_scan(doc, page, info) {
                // Labels come from the page's own text. Words that only /ActualText names (a stamp
                // drawn as outlines) are no labels, and leave the scan to be read.
                let own = crate::ocr::own_text(&mut r, page).ok();
                self.detect_on_scan(id, &mut r, page, own.as_ref(), &existing, &taken)
            } else {
                let res =
                    r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
                let words = res.text.map(|t| detection_words(&t, info)).unwrap_or_default();
                (pdfcraft_forms::detect::detect(&words, &shapes, &existing, &taken), 0)
            };
            for c in found {
                taken.push(c.name.clone());
                out.push((page, c, turn));
            }
        }
        out
    }

    /// Detect fields and add them, as one undoable step. Returns the new fields' names.
    pub fn auto_detect_fields(&mut self, id: DocId, pages: &[usize]) -> Result<Vec<String>, EditError> {
        let found = self.detect_fields_turned(id, pages);
        if found.is_empty() {
            return Ok(Vec::new());
        }
        let names = found.iter().map(|(_, c, _)| c.name.clone()).collect();
        let mut edits = Vec::with_capacity(found.len());
        for (page, c, turn) in found {
            let tall = if turn % 180 == 0 { c.rect[3] - c.rect[1] } else { c.rect[2] - c.rect[0] };
            let (kind, comb) = match c.kind {
                pdfcraft_forms::detect::Kind::Text => (pdfcraft_forms::NewField::Text { multiline: tall > 30.0 }, None),
                pdfcraft_forms::detect::Kind::CheckBox => (pdfcraft_forms::NewField::CheckBox, None),
                pdfcraft_forms::detect::Kind::Comb { cells } => (pdfcraft_forms::NewField::Text { multiline: false }, Some(cells)),
            };
            // The page already draws the line or box the field sits on, so the field gets no
            // border or fill of its own: an opaque fill would hide the printed form (#294, #2831).
            let mut props = FieldProps {
                appearance: Some(pdfcraft_forms::LookPatch { border: Some(None), fill: Some(None), ..Default::default() }),
                ..Default::default()
            };
            if turn != 0 {
                // A turned field keeps its place: the rectangle is already the one on the page.
                props.rotation = Some((0, turn));
                props.rect = Some((0, c.rect));
            }
            if let Some(cells) = comb {
                props.max_len = Some(Some(cells));
                props.flags = vec![(pdfcraft_forms::flags::COMB, true)];
            }
            edits.push(Edit::AddField { page, rect: c.rect, kind, name: Some(c.name.clone()) });
            edits.push(Edit::SetFieldProps { name: c.name, props: Box::new(props) });
        }
        self.apply(id, Edit::Batch { label: "Detect form fields".into(), edits })?;
        Ok(names)
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    /// A click returns its own alert when the output waiting in the document is full, and the
    /// waiting output stays.
    #[test]
    fn a_click_returns_its_alert_when_the_waiting_output_is_full() {
        let mut s = Session::new();
        let pdf = pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template());
        let id = s.open("xfa.pdf", None, std::sync::Arc::new(pdf), None).unwrap();
        s.take_js_output(id);
        s.docs.iter_mut().find(|d| d.id == id).unwrap().js_output.alert(vec![Message::plain("waiting"); MAX_ALERTS]);
        let o = s.run_javascript(id, "", Some("hello")).unwrap();
        assert_eq!(o.alerts.iter().map(|a| a.text.as_str()).collect::<Vec<_>>(), ["Hello 2"]);
        let out = s.take_js_output(id);
        assert_eq!(out.alerts, vec![Message::plain("waiting"); MAX_ALERTS]);
    }

    /// With the console full and closed, an operation still gets its own lines. Output of an
    /// operation that fails joins the waiting output.
    #[test]
    fn output_limits_count_only_the_operations_own_output() {
        let mut s = Session::new();
        let pdf = pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template());
        let id = s.open("xfa.pdf", None, std::sync::Arc::new(pdf), None).unwrap();
        s.take_js_output(id);
        fn output(s: &mut Session, id: DocId) -> &mut JsOutput {
            &mut s.docs.iter_mut().find(|d| d.id == id).unwrap().js_output
        }
        output(&mut s, id).say(vec!["waiting".to_string(); MAX_CONSOLE_LINES + 1]);
        let waiting = output(&mut s, id).console.clone();
        assert!(waiting.last().unwrap().starts_with("more console lines were left out"));
        let ((), own) = s
            .with_own_js_output::<_, ()>(id, |s| {
                output(s, id).say(["mine".to_string()]);
                output(s, id).errors.push("own error".into());
                Ok(())
            })
            .unwrap();
        assert_eq!(own.console, ["mine"]);
        assert_eq!(own.errors, ["own error"]);
        assert_eq!(output(&mut s, id).console, waiting);
        assert!(output(&mut s, id).errors.is_empty());
        let failed = s.with_own_js_output::<(), _>(id, |s| {
            output(s, id).errors.push("failed edit".into());
            Err("no")
        });
        assert_eq!(failed.unwrap_err(), "no");
        let out = s.take_js_output(id);
        assert_eq!(out.console, waiting);
        assert_eq!(out.errors, ["failed edit"]);
    }

    /// A form of many calculated fields, each printing its own cap of console lines, keeps a
    /// bounded console: past the budget, lines are left out and one closing line says so.
    #[test]
    fn console_output_of_one_edit_is_bounded() {
        let mut out = JsOutput::default();
        let line = "x".repeat(4_096);
        let run = Outcome { console: vec![line; 1_000], alerts: vec![Message::plain("hi"); 100], ..Default::default() };
        for _ in 0..1_000 {
            out.absorb(&run);
        }
        let bytes: usize = out.console.iter().map(String::len).sum();
        assert!(bytes <= MAX_CONSOLE_BYTES + 256, "{bytes} bytes kept");
        assert!(out.console.len() <= MAX_CONSOLE_LINES + 1, "{} lines kept", out.console.len());
        assert!(out.console.last().unwrap().starts_with("more console lines were left out"));
        assert_eq!(out.console.iter().filter(|l| l.starts_with("more console lines")).count(), 1, "said once");
        assert_eq!(out.alerts.len(), MAX_ALERTS);
        let mut merged = JsOutput::default();
        merged.append(out);
        assert!(merged.console.len() <= MAX_CONSOLE_LINES + 1);
    }

    /// A long run of underscores (a form's blank) is read in one pass: each glyph is compared with
    /// the word so far without reading the word again.
    #[test]
    fn a_long_underscore_run_is_read_in_one_pass() {
        let n = 10_000;
        let glyph = |i: usize, t: &str| pdfcraft_render::TextGlyph {
            text: t.into(),
            rect: [i as f32, 0.0, i as f32 + 1.0, 10.0],
            quad: [[0.0; 2]; 4],
            direction: [1.0, 0.0],
        };
        let mut glyphs: Vec<_> = (0..n).map(|i| glyph(i, "_")).collect();
        glyphs.push(glyph(n, "x"));
        // Built by hand, not read from a page the renderer cut short, so the text is complete.
        let text = pdfcraft_render::PageText { line_of: vec![0; glyphs.len()], space_before: vec![false; glyphs.len()], glyphs, truncated: false };
        UNDERSCORE_CHARS_READ.with(|c| c.set(0));
        let words = words_with(&text, |x, y| [x, y]);
        assert_eq!(words.iter().map(|(w, _)| w.len()).collect::<Vec<_>>(), [n, 1]);
        let read = UNDERSCORE_CHARS_READ.with(std::cell::Cell::get);
        assert!(read <= 4 * n, "{read} characters read for {n} glyphs");
    }

    /// Review on #3985: merging two outputs reports a failing document script once, as one run
    /// does, while each field script error stays: told apart by where an error came from, so a
    /// field script that throws a document script's very words is still reported.
    #[test]
    fn merged_output_reports_a_document_script_error_once() {
        let doc = "document script 2: TypeError: not a callable function";
        let field = "ReferenceError: x is not defined";
        let run = |error: &str| {
            let mut out = JsOutput::default();
            out.absorb(&Outcome { error: Some(error.into()), doc_errors: vec![doc.into()], ..Outcome::default() });
            out
        };
        let mut out = run(field);
        out.append(run(field));
        assert_eq!(out.errors, [field, doc, field]);
        // CodeRabbit on #3985: a field script that throws the document script's text.
        out.append(run(doc));
        assert_eq!(out.errors, [field, doc, field, doc]);
        let alone = run(doc);
        assert_eq!(alone.errors, [doc, doc], "the field's error and the document script's");
        let count = |o: &JsOutput| o.errors.iter().filter(|e| *e == doc).count();
        let mut merged = run(field);
        merged.append(alone);
        assert_eq!(count(&merged), 2, "the field's copy stays; the document script's is listed once");
    }

    #[test]
    fn duplicate_field_names_diff_against_first_original() {
        let first = pdfcraft_js::FieldState::new("duplicate", pdfcraft_js::FieldType::Text, vec!["first".into()]);
        let mut second = first.clone();
        second.value = vec!["second".into()];
        second.display = pdfcraft_js::DISPLAY_HIDDEN;
        let before = vec![first.clone(), second.clone()];
        let formatted = std::collections::BTreeSet::new();
        assert!(changes(&before, std::slice::from_ref(&first), &formatted).is_empty());
        let actual = changes(&before, std::slice::from_ref(&second), &formatted);
        assert_eq!(
            actual,
            vec![FieldChange {
                name: "duplicate".into(),
                value: Some(second.value),
                display: Some(pdfcraft_js::DISPLAY_HIDDEN),
                ..Default::default()
            }]
        );
        let outcome = pdfcraft_js::run_shared(
            "event.value = getField('duplicate').value; getField('duplicate').value = 'updated';",
            &pdfcraft_js::Event::field("Calculate", "duplicate", ""),
            &pdfcraft_js::DocInfo::default(),
            std::sync::Arc::new(before.clone()),
            &[],
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.value, "first");
        assert_eq!(outcome.changed.len(), 1);
        assert_eq!(
            changes(&before, &outcome.changed, &outcome.formatted),
            vec![FieldChange { name: "duplicate".into(), value: Some(vec!["updated".into()]), ..Default::default() }]
        );
        assert!(changes(&before, &[], &formatted).is_empty());
        let mut missing = first;
        missing.name = "missing".into();
        assert!(changes(&before, &[missing], &formatted).is_empty());
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;

    #[test]
    /// A setter matching widget zero still updates other widgets whose normal captions differ.
    fn setting_the_first_widgets_caption_still_emits_a_field_wide_change() {
        let mut button = pdfcraft_js::FieldState::new("go", pdfcraft_js::FieldType::Button, vec![]);
        button.caption = Some("First".into());
        button.caption_mixed = true;
        let before = vec![button];
        let outcome = pdfcraft_js::run(
            "getField('go').buttonSetCaption('First');",
            &pdfcraft_js::Event::field("Mouse Up", "go", ""),
            &pdfcraft_js::DocInfo { file_name: "buttons.pdf".into(), num_pages: 1, page: 0, info: vec![], xfa: false, ..Default::default() },
            &before,
            &[],
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        let changed = changes(&before, &outcome.changed, &outcome.formatted);
        assert_eq!(changed.len(), 1, "other widgets may have different captions");
        assert_eq!(changed[0].caption.as_deref(), Some("First"));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct PendingScript {
    pub id: DocId,
    generation: u64,
    runtime: Runtime,
    states: std::sync::Arc<Vec<pdfcraft_js::FieldState>>,
    pub receiver: std::sync::mpsc::Receiver<pdfcraft_js::interaction::InteractiveEvent>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Session {
    /// Check document identity/generation and the JavaScript preference before accepting a worker completion.
    pub fn script_pending_current(&self, p: &PendingScript) -> bool {
        !self.js_off && self.get(p.id).is_some_and(|d| d.generation == p.generation && std::sync::Arc::ptr_eq(&d.js_runtime, &p.runtime))
    }
    /// Capture the current fields/view and start a native script without blocking the caller on its prompts.
    pub fn start_javascript(&mut self, id: DocId, script: &str, target: Option<&str>) -> Result<PendingScript, EditError> {
        self.start_javascript_target(id, script, target, None)
    }
    /// Start an asynchronous button event retaining the clicked field's exact identity.
    pub fn start_javascript_target(
        &mut self,
        id: DocId,
        script: &str,
        target: Option<&str>,
        target_id: Option<u64>,
    ) -> Result<PendingScript, EditError> {
        if self.js_off {
            return Err(EditError::Invalid("JavaScript is disabled".into()));
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = &doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?.cos;
        let mut runner = JsRunner::new(cos, &doc.name, self.field_highlight);
        runner.doc.zoom = doc.js_zoom;
        runner.doc.page = doc.js_page;
        let fields = pdfcraft_forms::fields(cos);
        validate_script_target(target, target_id, &fields)?;
        let states = std::sync::Arc::new(fields.iter().map(|f| field_state(cos, f, &runner.doc)).collect::<Vec<_>>());
        let job = pdfcraft_js::session::Job {
            script: script.into(),
            event: target.map_or_else(
                || pdfcraft_js::Event::doc("Console"),
                |t| pdfcraft_js::Event { will_commit: false, target_id, ..pdfcraft_js::Event::field("Mouse Up", t, "") },
            ),
            doc: runner.doc,
            fields: states.clone(),
            doc_scripts: runner.doc_scripts,
            calculations: calculation_scripts(cos, &fields),
        };
        let receiver =
            doc.js_runtime.lock().map_err(|_| EditError::Invalid("script runtime unavailable".into()))?.start(job).map_err(EditError::Invalid)?;
        Ok(PendingScript { id, generation: doc.generation, runtime: doc.js_runtime.clone(), states, receiver })
    }
    /// Discard only the captured runtime, preserving a newer realm created by an intervening edit.
    pub fn discard_javascript(&mut self, p: &PendingScript) {
        if let Some(doc) = self.docs.iter_mut().find(|d| d.id == p.id)
            && std::sync::Arc::ptr_eq(&doc.js_runtime, &p.runtime)
        {
            replace_runtime(&mut doc.js_runtime);
        }
    }
    /// Discard a timed-out invocation and retain the worker slot until its thread exits.
    pub fn timeout_javascript(&mut self, p: PendingScript) {
        if self.script_pending_current(&p)
            && let Ok(mut runtime) = p.runtime.lock()
        {
            runtime.retire_timed_out_worker();
        } else {
            self.discard_javascript(&p);
        }
    }

    /// Apply a current worker result, or discard it and reset the realm when the document changed.
    pub fn finish_javascript(&mut self, p: PendingScript, out: Outcome) -> Result<Outcome, EditError> {
        if !self.script_pending_current(&p) {
            self.discard_javascript(&p);
            return Err(EditError::Invalid("document changed while the script was running".into()));
        }
        p.runtime.lock().map_err(|_| EditError::Invalid("script runtime unavailable".into()))?.acknowledge_completion();
        let discard = out.execution_refused && !out.realm_untouched;
        let result = self.apply_script_outcome(p.id, &p.states, out, false);
        if discard || result.is_err() {
            self.invalidate_javascript(p.id);
        }
        result
    }
}
pub use pdfcraft_js::interaction::{InteractiveEvent, MenuItem, Prompt};
impl Session {
    /// Supply current viewer state for script getters, clamping invalid zoom to the supported range.
    pub fn set_javascript_view(&mut self, id: DocId, zoom: f64, highlight: bool, page: usize) {
        self.set_field_highlight(highlight);
        if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
            doc.js_zoom = if zoom.is_finite() { zoom.clamp(1.0, 6400.0) } else { 100.0 };

            doc.js_page = page;
        }
    }
}

impl Session {
    /// Release this document runtime after cancellation or lifecycle changes; the next event initializes anew.
    pub fn invalidate_javascript(&mut self, id: DocId) {
        if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
            replace_runtime(&mut doc.js_runtime);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Page-tree snapshots and calculation-plan builds performed on this test thread.
    static SNAPSHOT_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
}

#[cfg(test)]
mod snapshot_review_tests {
    use super::*;

    #[test]
    fn document_actions_use_the_document_realm_and_obey_javascript_setting() {
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        let first = session.open("first.pdf", None, bytes.clone(), None).unwrap();
        let second = session.open("second.pdf", None, bytes, None).unwrap();
        session.run_javascript(first, "var counter = 4;", None).unwrap();

        for (event, name) in
            [(DocumentScriptEvent::CatalogOpen, "Open"), (DocumentScriptEvent::WillSave, "WillSave"), (DocumentScriptEvent::WillPrint, "WillPrint")]
        {
            let result = session.run_document_script_event(first, event, "[event.name,counter].join('/');").unwrap();
            assert_eq!(result.result, Some(format!("{name}/4")));
        }
        let result = session.run_document_script_event(second, DocumentScriptEvent::WillSave, "typeof counter;").unwrap();
        assert_eq!(result.result.as_deref(), Some("undefined"));
        session.set_javascript(false);
        assert!(session.run_document_script_event(first, DocumentScriptEvent::WillPrint, "counter++;").is_err());
    }

    /// Real page/info edits and completed Save As refresh metadata without losing script state.
    #[test]
    fn retained_document_metadata_tracks_edits_undo_and_save_as() {
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        let id = session.open("before.pdf", None, bytes, None).unwrap();
        assert_eq!(session.run_javascript(id, "var retained=41;numPages;", None).unwrap().result.as_deref(), Some("1"));
        session.apply(id, Edit::InsertBlankPage { at: 1, width: 200.0, height: 300.0 }).unwrap();
        session.apply(id, Edit::SetInfo { key: "Title".into(), value: "After".into() }).unwrap();
        let out = session.run_javascript(id, "[numPages,info.title,retained].join('/');", None).unwrap();
        assert_eq!(out.result.as_deref(), Some("2/After/41"));
        session.undo(id).unwrap();
        assert_eq!(session.run_javascript(id, "typeof info.title;", None).unwrap().result.as_deref(), Some("undefined"));
        session.redo(id).unwrap();
        let saved = session.save_bytes(id).unwrap();
        session.mark_saved(id, saved, Some("after.pdf".into())).unwrap();
        assert_eq!(
            session.run_javascript(id, "[documentFileName,numPages,info.title,retained].join('/');", None).unwrap().result.as_deref(),
            Some("after.pdf/2/After/41")
        );
        session.apply(id, Edit::DeletePages { pages: vec![1] }).unwrap();
        assert_eq!(session.run_javascript(id, "[numPages,retained].join('/');", None).unwrap().result.as_deref(), Some("1/41"));
        session.undo(id).unwrap();
        assert_eq!(session.run_javascript(id, "numPages;", None).unwrap().result.as_deref(), Some("2"));
        session.redo(id).unwrap();
        assert_eq!(session.run_javascript(id, "numPages;", None).unwrap().result.as_deref(), Some("1"));
    }

    /// A batch child that changes page structure or calculation sources invalidates cached plans.
    #[test]
    fn batch_children_refresh_geometry_metadata_and_calculation_sources() {
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        let id = session.open("batch.pdf", None, bytes, None).unwrap();
        for (i, name) in ["input", "total", "seen"].into_iter().enumerate() {
            session
                .apply(
                    id,
                    Edit::AddField {
                        page: 0,
                        rect: [20.0, 20.0 + i as f64 * 40.0, 120.0, 40.0 + i as f64 * 40.0],
                        kind: crate::NewField::Text { multiline: false },
                        name: Some(name.into()),
                    },
                )
                .unwrap();
        }
        session.apply(id, Edit::SetFieldValue { name: "input".into(), value: crate::FieldValue::Text("2".into()) }).unwrap();
        session
            .apply(
                id,
                Edit::SetFieldScript {
                    name: "total".into(),
                    event: "calculate".into(),
                    script: Some("event.value=Number(getField('input').value)+1;".into()),
                },
            )
            .unwrap();
        session
            .apply(
                id,
                Edit::SetFieldScript {
                    name: "input".into(),
                    event: "validate".into(),
                    script: Some("calculateNow();getField('seen').value=numPages+'/'+getField('total').value;".into()),
                },
            )
            .unwrap();
        session
            .apply(
                id,
                Edit::Batch {
                    label: "Structural batch".into(),
                    edits: vec![
                        Edit::SetFieldValue { name: "input".into(), value: crate::FieldValue::Text("3".into()) },
                        Edit::InsertBlankPage { at: 1, width: 200.0, height: 300.0 },
                        Edit::SetFieldScript {
                            name: "total".into(),
                            event: "calculate".into(),
                            script: Some("event.value=Number(getField('input').value)+10;".into()),
                        },
                        Edit::SetFieldValue { name: "input".into(), value: crate::FieldValue::Text("4".into()) },
                    ],
                },
            )
            .unwrap();
        let out = session.run_javascript(id, "getField('seen').value;", None).unwrap();
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("2/13"));
    }

    #[test]
    fn callback_field_cache_keeps_live_values_and_uses_object_identity() {
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        let id = session.open("cache.pdf", None, bytes, None).unwrap();
        for (name, rect) in [("first", [20.0, 20.0, 100.0, 40.0]), ("second", [20.0, 60.0, 100.0, 80.0])] {
            session.apply(id, Edit::AddField { page: 0, rect, kind: crate::NewField::Text { multiline: false }, name: Some(name.into()) }).unwrap();
        }

        let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
        let mut fields = pdfcraft_forms::fields(cos);
        fields[1].name = fields[0].name.clone();
        let mut runner = JsRunner::new(cos, "cache.pdf", false);
        let initial = runner.callback_fields(cos, &fields);
        assert_ne!(initial[0].object_id, initial[1].object_id);
        assert_ne!(initial[0].rect, initial[1].rect);
        fields[0].value = vec!["fresh".into()];
        let updated = runner.callback_fields(cos, &fields);
        assert_eq!(updated[0].value, ["fresh"]);
        assert_eq!(updated[1].value, initial[1].value);
        assert_eq!(runner.snapshot_fields.len(), 2);

        fields[0].widgets[0].rect = [20.0, 100.0, 100.0, 120.0];
        runner.invalidate_field_snapshots();
        let moved = runner.callback_fields(cos, &fields[..1]);
        assert_ne!(moved[0].rect, updated[0].rect);
        assert_eq!(runner.snapshot_fields.len(), 1);
    }

    /// A callback reuses the previous callback's snapshot, patching only values that changed,
    /// and a snapshot a caller still holds is never changed under it.
    #[test]
    fn callback_snapshots_are_patched_in_place_and_copied_only_when_shared() {
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        let id = session.open("patch.pdf", None, bytes, None).unwrap();
        for (name, rect) in [("first", [20.0, 20.0, 100.0, 40.0]), ("second", [20.0, 60.0, 100.0, 80.0])] {
            session.apply(id, Edit::AddField { page: 0, rect, kind: crate::NewField::Text { multiline: false }, name: Some(name.into()) }).unwrap();
        }
        let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
        let mut fields = pdfcraft_forms::fields(cos);
        let mut runner = JsRunner::new(cos, "patch.pdf", false);
        let first = runner.callback_fields(cos, &fields);
        let first_ptr = std::sync::Arc::as_ptr(&first);
        drop(first);
        // Nothing changed: the same snapshot comes back.
        let same = runner.callback_fields(cos, &fields);
        assert_eq!(std::sync::Arc::as_ptr(&same), first_ptr);
        drop(same);
        // A value changed and no one else holds the snapshot: it is patched in place.
        fields[1].value = vec!["7".into()];
        let patched = runner.callback_fields(cos, &fields);
        assert_eq!(std::sync::Arc::as_ptr(&patched), first_ptr);
        assert_eq!(patched[1].value, ["7"]);
        // A held snapshot keeps its values; the next callback gets a copy.
        fields[0].value = vec!["held".into()];
        let copied = runner.callback_fields(cos, &fields);
        assert_eq!(patched[0].value, Vec::<String>::new());
        assert_eq!(copied[0].value, ["held"]);
        assert_eq!(copied[1].value, ["7"]);
        // Instance tokens are stamped before dispatch, so the worker does not copy it again.
        assert!(copied.iter().all(|f| f.instance_id != 0));
    }

    /// Operation counts, rather than wall time, pin the absence of per-field tree/plan walks.
    #[test]
    fn field_events_reuse_page_geometry_and_calculation_plans() {
        let mut session = Session::new();
        let bytes = session.create_blank(600.0, 800.0, 4).unwrap();
        let id = session.open("synthetic.pdf", None, bytes, None).unwrap();
        for i in 0..12 {
            session
                .apply(
                    id,
                    Edit::AddField {
                        page: i % 4,
                        rect: [20.0, 20.0, 100.0, 40.0],
                        kind: crate::NewField::Text { multiline: false },
                        name: Some(format!("f{i}")),
                    },
                )
                .unwrap();
        }
        let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
        let mut fields = pdfcraft_forms::fields(cos);
        fields[1].actions.scripts.calculate = Some("event.value=Number(getField('f0').value)+1;".into());
        SNAPSHOT_WORK.with(|c| c.set((0, 0)));
        let mut runner = JsRunner::new(cos, "synthetic.pdf", false);
        for i in 0..8 {
            fields[0].value = vec![i.to_string()];
            let out = runner.run(FieldEvent::Calculate, "calculateNow();event.value=getField('f1').value;", &fields[1], "", &fields, cos);
            assert_eq!(out.value, (i + 1).to_string());
            assert!(runner.output.errors.is_empty(), "{:?}", runner.output.errors);
        }
        let work = SNAPSHOT_WORK.with(std::cell::Cell::get);
        eprintln!("geometry snapshots={}, calculation plan builds={}", work.0, work.1);
        assert_eq!(work, (1, 1), "one geometry snapshot and one calculation plan per runner");
    }
}

#[cfg(test)]
mod reset_budget_review_tests {
    use super::*;
    fn state(name: String) -> pdfcraft_js::FieldState {
        pdfcraft_js::FieldState::new(name, pdfcraft_js::FieldType::Text, Vec::new())
    }
    #[test]
    fn reset_budget_accepts_one_selected_field_in_a_large_form() {
        let states: Vec<_> = (0..1000).map(|i| state(format!("parent.field_{i:04}"))).collect();
        assert!(validate_reset_work(&[], &states, &[Request::Reset(vec!["parent.field_0001".into()])]).is_ok());
        assert!(validate_reset_work(&[], &states, &[Request::Reset(vec!["parent".into()])]).is_err());
    }

    #[test]
    fn reset_budget_charges_expanded_descendants() {
        let states: Vec<_> = (0..3000).map(|i| state(format!("parent.{i}"))).collect();
        assert!(validate_reset_work(&[], &states, &[Request::Reset(vec!["parent".into()])]).is_err());
        assert!(validate_reset_work(&[], &states, &[Request::Reset(Vec::new())]).is_ok());
    }
    #[test]
    fn reset_budget_charges_shared_prefix_comparisons() {
        let prefix = "p".repeat(4096);
        let states: Vec<_> = (0..128).map(|i| state(format!("{prefix}.{i}"))).collect();
        let names = (0..128).map(|i| format!("{prefix}.missing{i}")).collect();
        assert!(validate_reset_work(&[], &states, &[Request::Reset(names)]).is_err());
    }
    #[test]
    fn reset_budget_charges_full_form_name_copies() {
        let states = vec![state("n".repeat(140_000))];
        let resets = vec![Request::Reset(Vec::new()); 64];
        assert!(validate_reset_work(&[], &states, &resets).is_err());
        assert!(validate_reset_work(&[], &states, &[Request::Reset(Vec::new())]).is_ok());
    }
}

#[cfg(test)]
mod widget_budget_review_tests {
    use super::*;
    #[test]
    fn reset_budget_charges_all_widgets() {
        let widget = pdfcraft_forms::Widget {
            obj: pdfcraft_cos::ObjRef { num: 1, generation: 0 },
            page: None,
            rect: [0.0; 4],
            on_state: None,
            state: None,
            tab: 0,
            locked: false,
            hidden: false,
            read_only: false,
            rotation: 0,
            caption: None,
            mouse_up_script: None,
            list: None,
            round: false,
        };
        let mut field = Field {
            name: "x".into(),
            obj: widget.obj,
            kind: FieldKind::Text,
            value: vec![],
            default: vec![],
            flags: 0,
            max_len: None,
            options: vec![],
            da: String::new(),
            quadding: 0,
            tooltip: None,
            selected: vec![],
            single_value_is_array: false,
            widgets: vec![widget; 140_000],
            actions: Default::default(),
            button: None,
        };
        let resets = vec![Request::Reset(Vec::new()); 64];
        assert!(validate_reset_work(std::slice::from_ref(&field), &[], &resets).is_err());
        field.widgets.truncate(1);
        assert!(validate_reset_work(&[field], &[], &resets).is_ok());
    }
}

#[cfg(test)]
mod icon_mixed_review_tests {
    use super::*;
    /// Mixed widgets need a field-wide icon delta even when the first icon is already correct.
    #[test]
    fn mixed_icons_emit_only_after_an_explicit_setter() {
        for icon in [None, Some(7)] {
            let mut before = pdfcraft_js::FieldState::new("go", pdfcraft_js::FieldType::Button, vec![]);
            before.icon = icon;
            before.icon_mixed = true;
            let fields = vec![before.clone()];
            let script =
                if icon.is_some() { "getField('go').buttonSetIcon(getField('go').buttonGetIcon());" } else { "getField('go').buttonSetIcon(null);" };
            let out = pdfcraft_js::run(
                script,
                &pdfcraft_js::Event::doc("Test"),
                &pdfcraft_js::DocInfo::default(),
                &fields,
                &[],
                pdfcraft_js::Limits::default(),
            );
            assert_eq!(out.error, None);
            assert_eq!(changes(&[before.clone()], &out.changed, &out.formatted)[0].icon, Some(icon));
            assert!(changes(&[before.clone()], &[before], &Default::default()).is_empty());
        }
    }
}
