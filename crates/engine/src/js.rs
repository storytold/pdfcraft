//! Acrobat JavaScript in documents: field scripts run during form edits ([`JsRunner`], wired
//! into `forms` through its [`pdfcraft_forms::Scripts`] hook), and button actions and console
//! input run on request ([`Session::run_javascript`]). What scripts ask of the viewer (alerts,
//! printing, navigation, links) is kept for the front end in [`JsOutput`].
//!
//! Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript is [`Session::set_javascript`]; with it
//! off, scripts don't run and AF calls keep working natively.

use pdfcraft_forms::{Field, FieldChange, FieldEvent, FieldKind, FieldProps, ScriptResult, Scripts};
pub use pdfcraft_js::{Limits, Outcome, Panel, Request};

use crate::{DocId, Edit, EditError, Session};

/// What scripts produced for the viewer since it last looked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsOutput {
    pub alerts: Vec<String>,
    pub console: Vec<String>,
    pub requests: Vec<Request>,
    /// Script errors (as Acrobat's console shows them).
    pub errors: Vec<String>,
    /// Which of `errors` are document scripts' (#3940), told apart by where they came from and
    /// not by their text, which a field script's error can share: each is reported once, however
    /// often the document scripts run again.
    pub doc_errors: Vec<String>,
}

impl JsOutput {
    /// Whether scripts produced nothing: no alerts, console lines, requests or errors.
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty() && self.console.is_empty() && self.requests.is_empty() && self.errors.is_empty()
    }

    /// Adds what a field's own script produced: its alerts, console lines, errors and requests,
    /// except a request to show a side panel, open Save As or open another document, which
    /// is noted on the console and dropped.
    fn absorb(&mut self, o: &Outcome) {
        self.alerts.extend(o.alerts.iter().cloned());
        self.console.extend(o.console.iter().cloned());
        // A field's own scripts (keystroke, format, validate, calculate) run unasked: only a
        // button's may show or hide a side panel, or open Save As.
        // Opening another document also requires an explicit action, not an implicit field event.
        for r in &o.requests {
            match r {
                Request::ShowPanel(_) => self.console.push("A field script asked to show a side panel; only a button may show a side panel".into()),
                Request::SaveAs => {
                    self.console.push("A field script asked to save the document as a new file; only a button may open Save As".into())
                }
                Request::OpenDoc(_) => {
                    self.console.push("A field script asked to open another document; opening documents requires an explicit action".into())
                }
                r => self.requests.push(r.clone()),
            }
        }
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
        self.alerts.extend(other.alerts);
        self.console.extend(other.console);
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

/// A form field as scripts see it.
pub fn field_state(doc: &pdfcraft_cos::Document, f: &Field) -> pdfcraft_js::FieldState {
    field_state_with_display(f, display(doc, f))
}

fn field_state_with_display(f: &Field, display: i32) -> pdfcraft_js::FieldState {
    let mut s = pdfcraft_js::FieldState::new(f.name.clone(), kind(f.kind), f.value.clone());
    s.default = f.default.clone();
    s.readonly = f.read_only();
    s.required = f.has(pdfcraft_forms::flags::REQUIRED);
    s.display = display;
    s.char_limit = f.max_len;
    s.options = match f.kind {
        FieldKind::CheckBox | FieldKind::Radio => f.widgets.iter().filter_map(|w| w.on_state.clone()).map(|o| (o.clone(), o)).collect(),
        _ => f.options.clone(),
    };
    s
}

/// The changes a script made, against the fields as they were.
fn changes(before: &[pdfcraft_js::FieldState], after: &[pdfcraft_js::FieldState]) -> Vec<FieldChange> {
    if after.is_empty() {
        return Vec::new();
    }
    // Outcomes contain only fields touched by the script. Index the original snapshot once,
    // keeping the first match as the old linear lookup did for malformed duplicate names.
    let mut by_name = std::collections::HashMap::with_capacity(before.len());
    for field in before {
        by_name.entry(field.name.as_str()).or_insert(field);
    }
    after
        .iter()
        .filter_map(|a| {
            let b = by_name.get(a.name.as_str())?;
            let c = FieldChange {
                name: a.name.clone(),
                value: (a.value != b.value).then(|| a.value.clone()),
                read_only: (a.readonly != b.readonly).then_some(a.readonly),
                required: (a.required != b.required).then_some(a.required),
                display: (a.display != b.display).then_some(a.display),
            };
            (c.value.is_some() || c.read_only.is_some() || c.required.is_some() || c.display.is_some()).then_some(c)
        })
        .collect()
}

/// Runs field scripts with the document's object model and keeps what they print or ask for.
pub struct JsRunner {
    pub doc: pdfcraft_js::DocInfo,
    pub doc_scripts: Vec<String>,
    pub output: JsOutput,
    /// Immutable document snapshot from the start of the edit (for widget `display`).
    cos: pdfcraft_cos::Document,
    // `cos` is the same immutable pre-edit snapshot for the runner's lifetime. Cache only
    // its scalar display readings, not COS objects: large forms exceed the object cache and
    // otherwise reparse every widget for every calculation. Values/flags still come from
    // the live `fields` snapshot on each call. A new edit gets a new runner and cache.
    displays: std::collections::HashMap<pdfcraft_cos::ObjRef, i32>,
}

impl JsRunner {
    /// A runner for `cos`: what scripts read about the document (pages, info, whether it is
    /// an XFA form) and its document-level scripts, read once.
    pub fn new(cos: &pdfcraft_cos::Document, file_name: &str) -> JsRunner {
        let xfa = pdfcraft_xfa::xfa_shape(cos).is_some();
        let doc =
            pdfcraft_js::DocInfo { file_name: file_name.to_string(), num_pages: pdfcraft_model::pages(cos).len(), page: 0, info: info(cos), xfa };
        // Once XFA owns the form, the PDF container's scripts belong to its fallback view.
        // Keep them in the file and in Document JavaScripts, but do not initialize an
        // AcroForm context with them: a viewer-compatibility alert would be misleading.
        // Packets alone are not enough: static XFA and an unlaid-out shell keep their scripts.
        let xfa_active = xfa && pdfcraft_xfa::existing_layout(cos).is_some();
        let doc_scripts = if xfa_active { Vec::new() } else { pdfcraft_forms::document_scripts(cos) };
        JsRunner { doc, doc_scripts, output: JsOutput::default(), cos: cos.clone(), displays: std::collections::HashMap::new() }
    }
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
    fn run(&mut self, event: FieldEvent, script: &str, target: &Field, value: &str, fields: &[Field]) -> ScriptResult {
        let states: Vec<_> = fields
            .iter()
            .map(|f| {
                let display = match f.widgets.first() {
                    Some(w) => *self.displays.entry(w.obj).or_insert_with(|| display(&self.cos, f)),
                    None => pdfcraft_js::DISPLAY_VISIBLE,
                };
                field_state_with_display(f, display)
            })
            .collect();
        let states = std::sync::Arc::new(states);
        let ev = pdfcraft_js::Event::field(event.name(), &target.name, value);
        let o = pdfcraft_js::run_shared(script, &ev, &self.doc, std::sync::Arc::clone(&states), &self.doc_scripts, Limits::default());
        self.output.absorb(&o);
        if o.error.is_some() {
            // As in Acrobat, a failing script leaves the value alone (the error goes to the console).
            return ScriptResult { rc: true, value: value.to_string(), changes: Vec::new(), message: None };
        }
        ScriptResult {
            rc: o.rc,
            value: o.value.clone(),
            changes: changes(&states, &o.changed),
            message: (!o.rc).then(|| o.alerts.last().cloned()).flatten(),
        }
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

impl Session {
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript (on by default).
    pub fn set_javascript(&mut self, on: bool) {
        self.js_off = !on;
    }

    pub fn javascript(&self) -> bool {
        !self.js_off
    }

    /// What scripts printed or asked for since the last call (alerts, console, print, …).
    pub fn take_js_output(&mut self, id: DocId) -> JsOutput {
        self.docs.iter_mut().find(|d| d.id == id).map(|d| std::mem::take(&mut d.js_output)).unwrap_or_default()
    }

    /// Run `script` in document `id`, for a button (`target`: the field, event "Mouse Up") or
    /// the console (no target). Field changes and `resetForm` are applied as one undoable step;
    /// everything else the script asked for is returned.
    /// The ordinary button/console path applies collected edits even when `Outcome::error`
    /// is set: a script error is returned in the outcome, not a promise of rollback.
    /// Edit failures instead return `EditError`. Link actions use
    /// [`Self::finish_link_javascript`], which discards failed scripts' edits and requests.
    pub fn run_javascript(&mut self, id: DocId, script: &str, target: Option<&str>) -> Result<Outcome, EditError> {
        if self.js_off {
            return Err(EditError::Invalid("JavaScript is turned off (Preferences ▸ JavaScript)".into()));
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = doc.editor.as_ref().map(|e| e.cos.clone()).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        // A button PdfCraft generated from an XFA template runs its XFA click script.
        if let Some(t) = target
            && let Some(tpl) = doc.xfa_template.clone()
            && let Some(field) = doc.form.iter().find(|f| f.name == t)
            && let Some(som) = crate::xfa::clickable_som(&cos, &tpl, field)
        {
            self.apply(id, Edit::XfaEvent { som, activity: "click".into() })?;
            let out = self.take_js_output(id);
            return Ok(Outcome {
                rc: true,
                alerts: out.alerts,
                console: out.console,
                requests: out.requests,
                error: out.errors.first().cloned(),
                ..Default::default()
            });
        }
        let runner = JsRunner::new(&cos, &doc.name);
        let fields = pdfcraft_forms::fields(&cos);
        let states: Vec<_> = fields.iter().map(|f| field_state(&cos, f)).collect();
        let event = match target {
            Some(t) => pdfcraft_js::Event { will_commit: false, ..pdfcraft_js::Event::field("Mouse Up", t, "") },
            None => pdfcraft_js::Event::doc("Console"),
        };
        let mut o = pdfcraft_js::run(script, &event, &runner.doc, &states, &runner.doc_scripts, Limits::default());
        self.apply_javascript_outcome(id, &states, &fields, &mut o)?;
        Ok(o)
    }

    /// Submit field changes and `resetForm` requests as one permission-checked Undo batch.
    /// `states` and `fields` must describe the source snapshot used to produce `o`.
    /// The caller decides whether that outcome is eligible: this helper checks neither
    /// its error nor the source identity/generation. Link completion rejects failed or
    /// stale results before calling it; the synchronous runner has a different error policy.
    /// Reset requests are consumed before submitting the batch, even if the edit fails;
    /// other viewer requests stay in `o`. Edit failures propagate from `Session::apply`.
    fn apply_javascript_outcome(
        &mut self,
        id: DocId,
        states: &[pdfcraft_js::FieldState],
        fields: &[Field],
        o: &mut Outcome,
    ) -> Result<(), EditError> {
        let changes = changes(states, &o.changed);
        let resets: Vec<Vec<String>> = o
            .requests
            .iter()
            .filter_map(|r| match r {
                Request::Reset(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        o.requests.retain(|r| !matches!(r, Request::Reset(_)));
        let mut edits = Vec::new();
        if !changes.is_empty() {
            edits.push(Edit::ApplyScriptChanges { changes });
        }
        for names in resets {
            let all: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
            let names = (!names.is_empty())
                .then(|| all.iter().filter(|n| names.iter().any(|x| *n == x || n.starts_with(&format!("{x}.")))).cloned().collect());
            edits.push(Edit::ResetForm { names });
        }
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: "Run JavaScript".into(), edits })?;
        }
        Ok(())
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
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let shapes = pdfcraft_forms::detect::page_shapes(cos, page);
            let existing: Vec<[f64; 4]> = doc.form.iter().flat_map(|f| f.widgets.iter()).filter(|w| w.page == Some(page)).map(|w| w.rect).collect();
            let (found, turn) = if shapes.boxes.is_empty() && shapes.rules.is_empty() && crate::scan_fields::is_scan(doc, page, info) {
                self.detect_on_scan(id, &mut r, page, res.text.as_deref(), &existing, &taken)
            } else {
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

pub use pdfcraft_js::{InteractiveEvent, MenuItem};

/// Immutable input for an annotation action. No files or UI are accessed by the worker.
pub struct LinkScript {
    id: DocId,
    script: String,
    event: pdfcraft_js::Event,
    doc: pdfcraft_js::DocInfo,
    fields: Vec<pdfcraft_js::FieldState>,
    document_scripts: Vec<String>,
    generation: u64,
    form: Vec<Field>,
}

impl LinkScript {
    /// The source document captured at preparation, independent of the current UI view.
    pub fn document_id(&self) -> DocId {
        self.id
    }
    /// Start a fresh worker from this snapshot; no document effects are applied here.
    /// Reply to each menu (or drop its reply sender), then pass completion to
    /// [`Session::finish_link_javascript`]. Closing the channel does not terminate the VM.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(&self) -> Result<std::sync::mpsc::Receiver<InteractiveEvent>, String> {
        pdfcraft_js::interactive(self.script.clone(), self.event.clone(), self.doc.clone(), self.fields.clone(), self.document_scripts.clone())
    }

    /// Headless execution cancels menus by default; requests are returned, never carried out.
    pub fn run(&self) -> Outcome {
        pdfcraft_js::run(&self.script, &self.event, &self.doc, &self.fields, &self.document_scripts, Limits::default())
    }
}

impl Session {
    /// Resolve a link by its zero-based page and raw annotation index.
    /// Capture fields, source identity and generation without running any script.
    /// Document initializers follow `JsRunner`'s policy: PDF fallback scripts are omitted
    /// when XFA layout owns the form, without removing them from the document.
    pub fn prepare_link_javascript(&self, id: DocId, page: usize, index: usize) -> Result<LinkScript, EditError> {
        if !self.javascript() {
            return Err(EditError::Invalid("JavaScript is turned off".into()));
        }
        let d = self.get(id).ok_or(EditError::NoDocument)?;
        let link = d.info.links.iter().find(|l| l.page == page && l.index == index).ok_or_else(|| EditError::Invalid("link not found".into()))?;
        let pdfcraft_render::LinkTarget::JavaScript(script) = &link.target else {
            return Err(EditError::Invalid("link has no supported JavaScript action".into()));
        };
        let cos = match &d.editor {
            Some(editor) => editor.cos.clone(),
            None => {
                pdfcraft_cos::Document::open_with_password(d.bytes.clone(), d.password.as_deref()).map_err(|e| EditError::Invalid(e.to_string()))?
            }
        };
        let runner = JsRunner::new(&cos, &d.name);
        let form = pdfcraft_forms::fields(&cos);
        let fields = form.iter().map(|f| field_state(&cos, f)).collect();
        let mut doc = runner.doc;
        doc.page = page;
        Ok(LinkScript {
            id,
            script: script.clone(),
            event: pdfcraft_js::Event { kind: "Link".into(), ..pdfcraft_js::Event::doc("Mouse Up") },
            doc,
            fields,
            document_scripts: runner.doc_scripts,
            generation: d.generation,
            form,
        })
    }

    /// Whether this session still has the unchanged source document with JavaScript enabled.
    /// Frontends separately check which document is active; view geometry is not engine state.
    pub fn link_javascript_is_current(&self, input: &LinkScript) -> bool {
        self.javascript() && self.get(input.id).is_some_and(|doc| doc.generation == input.generation)
    }

    /// Commit only to the unchanged source document and only while JavaScript is enabled.
    /// Successful edits use the common permission-checked Undo batch. Failed scripts keep
    /// their diagnostics but commit no edits or requests; callers dispatch returned requests.
    pub fn finish_link_javascript(&mut self, input: &LinkScript, mut out: Outcome) -> Result<Outcome, EditError> {
        if !self.javascript() {
            return Err(EditError::Invalid("JavaScript is turned off".into()));
        }
        let d = self.get(input.id).ok_or(EditError::NoDocument)?;
        if d.generation != input.generation {
            return Err(EditError::Invalid("document changed while JavaScript was running".into()));
        }
        if out.error.is_none() {
            self.apply_javascript_outcome(input.id, &input.fields, &input.form, &mut out)?;
        } else {
            out.requests.clear();
        }
        Ok(out)
    }
}

/// Resolve Acrobat device-independent paths through the existing local-PDF policy. No I/O.
pub fn resolve_javascript_document_path(source: Option<&std::path::Path>, raw: &str) -> Result<std::path::PathBuf, crate::RemoteDocumentPathError> {
    #[cfg(windows)]
    let normalized = {
        let b = raw.as_bytes();
        if b.first() == Some(&b'/') && b.get(1).is_some_and(u8::is_ascii_alphabetic) {
            if b.get(2) == Some(&b'/') {
                Some(format!("{}:{}", char::from(*b.get(1).unwrap_or(&b'C')), raw.get(2..).unwrap_or_default()))
            } else if b.get(2) == Some(&b':') && b.get(3) == Some(&b'/') {
                raw.get(1..).map(str::to_string)
            } else {
                None
            }
        } else {
            None
        }
    };
    #[cfg(windows)]
    let raw = normalized.as_deref().unwrap_or(raw);
    crate::resolve_remote_document_path(source, raw)
}

#[cfg(test)]
mod review_tests {
    use super::*;

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
        let text = pdfcraft_render::PageText { line_of: vec![0; glyphs.len()], space_before: vec![false; glyphs.len()], glyphs };
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
        assert!(changes(&before, std::slice::from_ref(&first)).is_empty());
        let actual = changes(&before, std::slice::from_ref(&second));
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
            changes(&before, &outcome.changed),
            vec![FieldChange { name: "duplicate".into(), value: Some(vec!["updated".into()]), ..Default::default() }]
        );
        assert!(changes(&before, &[]).is_empty());
        let mut missing = first;
        missing.name = "missing".into();
        assert!(changes(&before, &[missing]).is_empty());
    }
}
