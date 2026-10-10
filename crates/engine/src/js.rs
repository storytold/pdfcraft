//! Acrobat JavaScript in documents: field scripts run during form edits ([`JsRunner`], wired
//! into `forms` through its [`pdfcraft_forms::Scripts`] hook), and button actions and console
//! input run on request ([`Session::run_javascript`]). What scripts ask of the viewer (alerts,
//! printing, navigation, links) is kept for the front end in [`JsOutput`].
//!
//! Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript is [`Session::set_javascript`]; with it
//! off, scripts don't run and AF calls keep working natively.

use pdfcraft_forms::{Field, FieldChange, FieldEvent, FieldKind, ScriptResult, Scripts};
pub use pdfcraft_js::{Limits, Outcome, Request};

use crate::{DocId, Edit, EditError, Session};

/// What scripts produced for the viewer since it last looked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsOutput {
    pub alerts: Vec<String>,
    pub console: Vec<String>,
    pub requests: Vec<Request>,
    /// Script errors (as Acrobat's console shows them).
    pub errors: Vec<String>,
}

impl JsOutput {
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty() && self.console.is_empty() && self.requests.is_empty() && self.errors.is_empty()
    }

    fn absorb(&mut self, o: &Outcome) {
        self.alerts.extend(o.alerts.iter().cloned());
        self.console.extend(o.console.iter().cloned());
        self.requests.extend(o.requests.iter().cloned());
        self.errors.extend(o.error.iter().cloned());
    }

    pub(crate) fn append(&mut self, other: JsOutput) {
        self.alerts.extend(other.alerts);
        self.console.extend(other.console);
        self.requests.extend(other.requests);
        self.errors.extend(other.errors);
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
    let mut s = pdfcraft_js::FieldState::new(f.name.clone(), kind(f.kind), f.value.clone());
    s.default = f.default.clone();
    s.readonly = f.read_only();
    s.required = f.has(pdfcraft_forms::flags::REQUIRED);
    s.display = display(doc, f);
    s.char_limit = f.max_len;
    s.options = match f.kind {
        FieldKind::CheckBox | FieldKind::Radio => f.widgets.iter().filter_map(|w| w.on_state.clone()).map(|o| (o.clone(), o)).collect(),
        _ => f.options.clone(),
    };
    s.actions = widget_scripts(doc, f);
    for (name, script) in [
        ("Keystroke", &f.actions.scripts.keystroke),
        ("Validate", &f.actions.scripts.validate),
        ("Calculate", &f.actions.scripts.calculate),
        ("Format", &f.actions.scripts.format),
    ] {
        if let Some(script) = script {
            s.actions.push((name.into(), script.clone()));
        }
    }
    if let Some(w) = f.widgets.first() {
        let mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK")).map(|m| doc.resolve(m));
        let color = mk.as_ref().and_then(|m| m.as_dict()).and_then(|m| m.get(b"BC")).map(|c| doc.resolve(c));
        if let Some(values) = color.as_ref().and_then(|c| c.as_array()) {
            let space = match values.len() {
                1 => "G",
                3 => "RGB",
                4 => "CMYK",
                _ => "T",
            };
            let mut parts = vec![space.into()];
            parts.extend(values.iter().filter_map(|v| v.as_f64()).map(|v| v.to_string()));
            s.stroke_color = Some(parts);
        }
    }
    s
}

fn widget_scripts(doc: &pdfcraft_cos::Document, f: &Field) -> Vec<(String, String)> {
    use pdfcraft_cos::Object;
    let Some(w) = f.widgets.first() else { return Vec::new() };
    let widget = doc.get(w.obj);
    let Some(d) = widget.as_dict() else { return Vec::new() };
    let aa = d.get(b"AA").map(|a| doc.resolve(a));
    let aa = aa.as_ref().and_then(|a| a.as_dict());
    let mut out = Vec::new();
    for (name, key) in
        [("MouseUp", b"U".as_slice()), ("MouseDown", b"D"), ("MouseEnter", b"E"), ("MouseExit", b"X"), ("OnFocus", b"Fo"), ("OnBlur", b"Bl")]
    {
        let action = if name == "MouseUp" { d.get(b"A").or_else(|| aa.and_then(|a| a.get(key))) } else { aa.and_then(|a| a.get(key)) };
        let Some(action) = action.map(|a| doc.resolve(a)) else { continue };
        let Some(action) = action.as_dict().filter(|a| a.name(b"S") == Some(b"JavaScript")) else { continue };
        let Some(source) = action.get(b"JS").map(|s| doc.resolve(s)) else { continue };
        let source = match &*source {
            Object::String(s) => s.to_text(),
            Object::Stream(s) => match s.decoded() {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => continue,
            },
            _ => continue,
        };
        out.push((name.into(), source));
    }
    out
}

/// The changes a script made, against the fields as they were.
fn changes(before: &[pdfcraft_js::FieldState], after: &[pdfcraft_js::FieldState]) -> Vec<FieldChange> {
    after
        .iter()
        .filter_map(|a| {
            let b = before.iter().find(|b| b.name == a.name)?;
            let c = FieldChange {
                name: a.name.clone(),
                value: (a.value != b.value).then(|| a.value.clone()),
                read_only: (a.readonly != b.readonly).then_some(a.readonly),
                required: (a.required != b.required).then_some(a.required),
                display: (a.display != b.display).then_some(a.display),
                options: (a.options != b.options).then(|| a.options.clone()),
                stroke_color: (a.stroke_color != b.stroke_color).then(|| a.stroke_color.clone()).flatten(),
                actions: a.actions.iter().filter(|action| !b.actions.contains(action)).cloned().collect(),
            };
            (c.value.is_some()
                || c.read_only.is_some()
                || c.required.is_some()
                || c.display.is_some()
                || c.options.is_some()
                || c.stroke_color.is_some()
                || !c.actions.is_empty())
            .then_some(c)
        })
        .collect()
}

/// Runs field scripts with the document's object model and keeps what they print or ask for.
pub struct JsRunner {
    pub doc: pdfcraft_js::DocInfo,
    pub doc_scripts: Vec<String>,
    pub output: JsOutput,
    /// The fields in the document as the forms code last read them (for `display`).
    cos: pdfcraft_cos::Document,
    runtime: pdfcraft_js::Runtime,
}

impl JsRunner {
    pub fn new(cos: &pdfcraft_cos::Document, file_name: &str, runtime: pdfcraft_js::Runtime) -> JsRunner {
        let doc = pdfcraft_js::DocInfo { file_name: file_name.to_string(), num_pages: pdfcraft_model::pages(cos).len(), page: 0, info: info(cos) };
        JsRunner { doc, doc_scripts: pdfcraft_forms::document_scripts(cos), output: JsOutput::default(), cos: cos.clone(), runtime }
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
        let states: Vec<_> = fields.iter().map(|f| field_state(&self.cos, f)).collect();
        let ev = pdfcraft_js::Event::field(event.name(), &target.name, value);
        let o = self.runtime.run(script, &ev, &self.doc, &states, &self.doc_scripts, Limits::default());
        self.output.absorb(&o);
        if o.error.is_some() {
            // As in Acrobat, a failing script leaves the value alone (the error goes to the console).
            return ScriptResult { rc: true, value: value.to_string(), changes: Vec::new(), message: None };
        }
        let after: Vec<_> = states.iter().map(|s| o.changed.iter().find(|c| c.name == s.name).unwrap_or(s).clone()).collect();
        ScriptResult {
            rc: o.rc,
            value: o.value.clone(),
            changes: changes(&states, &after),
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
    pub fn run_javascript(&mut self, id: DocId, script: &str, target: Option<&str>) -> Result<Outcome, EditError> {
        let event = match target {
            Some(t) => pdfcraft_js::Event { will_commit: false, ..pdfcraft_js::Event::field("Mouse Up", t, "") },
            None => pdfcraft_js::Event::doc("Console"),
        };
        self.run_js_event(id, script, event)
    }

    fn run_js_event(&mut self, id: DocId, script: &str, event: pdfcraft_js::Event) -> Result<Outcome, EditError> {
        if self.js_off {
            return Err(EditError::Invalid("JavaScript is turned off (Preferences ▸ JavaScript)".into()));
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = doc.editor.as_ref().map(|e| e.cos.clone()).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        // A button PdfCraft generated from an XFA template runs its XFA click script.
        if let Some(t) = event.target.as_deref()
            && let Some(tpl) = doc.xfa_template.clone()
            && let Some(som) = crate::xfa::clickable_som(&cos, &tpl, t)
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
        let runtime = self.js_runtime(id)?;
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let runner = JsRunner::new(&cos, &doc.name, runtime);
        let fields = pdfcraft_forms::fields(&cos);
        let states: Vec<_> = fields.iter().map(|f| field_state(&cos, f)).collect();
        let mut o = runner.runtime.run(script, &event, &runner.doc, &states, &runner.doc_scripts, Limits::default());
        let after: Vec<_> = states.iter().map(|s| o.changed.iter().find(|c| c.name == s.name).unwrap_or(s).clone()).collect();
        let changes = changes(&states, &after);
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
        Ok(o)
    }

    pub(crate) fn js_runtime(&mut self, id: DocId) -> Result<pdfcraft_js::Runtime, EditError> {
        let doc = self.doc_mut(id)?;
        if doc.js_runtime.is_none() {
            doc.js_runtime = Some(pdfcraft_js::Runtime::new().map_err(EditError::Invalid)?);
        }
        doc.js_runtime.clone().ok_or_else(|| EditError::Invalid("the script engine could not start".into()))
    }

    /// Dispatch a stored widget JavaScript action, using the same realm as document scripts.
    pub fn run_field_event(&mut self, id: DocId, field: &str, trigger: pdfcraft_forms::Trigger) -> Result<JsOutput, EditError> {
        if self.js_off {
            return Ok(JsOutput::default());
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = doc.editor.as_ref().map(|e| &e.cos).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let target =
            pdfcraft_forms::fields(cos).into_iter().find(|f| f.name == field).ok_or_else(|| EditError::Invalid(format!("no field named {field}")))?;
        let scripts = pdfcraft_forms::field_actions(cos, field)?;
        let mut output = JsOutput::default();
        for (t, action) in scripts {
            if t == trigger
                && let pdfcraft_forms::FieldAction::JavaScript(script) = action
            {
                let name = match trigger {
                    pdfcraft_forms::Trigger::OnFocus => "Focus",
                    pdfcraft_forms::Trigger::OnBlur => "Blur",
                    _ => trigger.label(),
                };
                let event = pdfcraft_js::Event { will_commit: false, ..pdfcraft_js::Event::field(name, field, &target.value.join(",")) };
                let out = self.run_js_event(id, &script, event)?;
                output.absorb(&out);
            }
        }
        Ok(output)
    }

    /// Document lifecycle actions share the same realm as field and console scripts.
    pub fn run_document_event(&mut self, id: DocId, name: &str) -> Result<(), EditError> {
        if self.js_off {
            return Ok(());
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return Ok(()) };
        let scripts = lifecycle_scripts(cos, name);
        if scripts.is_empty() && (name != "Open" || pdfcraft_forms::document_scripts(cos).is_empty()) {
            return Ok(());
        }
        // Even with no OpenAction, evaluate document scripts once on opening.
        let scripts = if scripts.is_empty() { vec![String::new()] } else { scripts };
        for script in scripts {
            let out = self.run_js_event(id, &script, pdfcraft_js::Event::doc(name))?;
            self.doc_mut(id)?.js_output.absorb(&out);
            if let Some(error) = out.error
                && name == "WillSave"
            {
                return Err(EditError::Invalid(format!("Will Save script failed: {error}")));
            }
        }
        Ok(())
    }
}

fn lifecycle_scripts(cos: &pdfcraft_cos::Document, name: &str) -> Vec<String> {
    use pdfcraft_cos::Object;
    let Some(root) = cos.trailer().get(b"Root").map(|o| cos.resolve(o)) else { return Vec::new() };
    let Some(catalog) = root.as_dict() else { return Vec::new() };
    let action = if name == "Open" {
        catalog.get(b"OpenAction").cloned()
    } else {
        let key: &[u8] = match name {
            "WillSave" => b"WS",
            "DidSave" => b"DS",
            "WillPrint" => b"WP",
            "DidPrint" => b"DP",
            _ => return Vec::new(),
        };
        catalog.get(b"AA").map(|a| cos.resolve(a)).and_then(|a| a.as_dict().and_then(|d| d.get(key)).cloned())
    };
    let mut pending: Vec<_> = action.into_iter().collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut count = 0;
    while let Some(action) = pending.pop() {
        count += 1;
        if count > 256 {
            break;
        }
        if let Some(r) = action.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        let action = cos.resolve(&action);
        if let Some(array) = action.as_array() {
            pending.extend(array.iter().rev().take(256).cloned());
            continue;
        }
        let Some(d) = action.as_dict() else { continue };
        if d.name(b"S") == Some(b"JavaScript")
            && let Some(js) = d.get(b"JS")
        {
            match &*cos.resolve(js) {
                Object::String(s) => out.push(s.to_text()),
                Object::Stream(s) => {
                    if let Ok(bytes) = s.decoded() {
                        out.push(String::from_utf8_lossy(&bytes).into_owned());
                    }
                }
                _ => {}
            }
        }
        if let Some(next) = d.get(b"Next") {
            pending.push(next.clone());
        }
    }
    out
}

/// A page's words for form-field detection, in user space (underscore runs split from the
/// text around them).
fn detection_words(text: &pdfcraft_render::PageText, info: &pdfcraft_render::PageInfo) -> Vec<pdfcraft_forms::detect::Word> {
    page_words(text, info).into_iter().map(|(text, rect)| pdfcraft_forms::detect::Word { text, rect }).collect()
}

/// A page's words in reading order, in user space ([x0, y0, x1, y1]); runs of underscores are
/// words of their own.
pub(crate) fn page_words(text: &pdfcraft_render::PageText, info: &pdfcraft_render::PageInfo) -> Vec<(String, [f64; 4])> {
    let mut words: Vec<(String, [f32; 4])> = Vec::new();
    let mut last_line = u32::MAX;
    // A space glyph ends the word before it.
    let mut gap = false;
    for (i, g) in text.glyphs.iter().enumerate() {
        let line = text.line_of.get(i).copied().unwrap_or(0);
        if g.text.trim().is_empty() {
            gap = true;
            continue;
        }
        let under = g.text.chars().all(|c| c == '_');
        let breaks = match words.last() {
            None => true,
            Some((w, _)) => gap || line != last_line || text.space_before.get(i).copied().unwrap_or(false) || w.chars().all(|c| c == '_') != under,
        };
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
            _ => words.push((g.text.clone(), g.rect)),
        }
    }
    words
        .into_iter()
        .map(|(text, r)| {
            let a = info.view_to_user(r[0], r[1]);
            let b = info.view_to_user(r[2], r[3]);
            let rect = [a[0].min(b[0]) as f64, a[1].min(b[1]) as f64, a[0].max(b[0]) as f64, a[1].max(b[1]) as f64];
            (text, rect)
        })
        .collect()
}

impl Session {
    /// Prepare a form ▸ automatic field detection: the fields `pages` (0-based; empty = all)
    /// seem to ask for, named from their labels, as (page, candidate).
    pub fn detect_fields(&self, id: DocId, pages: &[usize]) -> Vec<(usize, pdfcraft_forms::detect::Candidate)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return Vec::new() };
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut taken: Vec<String> = doc.form.iter().map(|f| f.name.clone()).collect();
        let all = doc.info.pages.len();
        let list: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.iter().copied().filter(|p| *p < all).collect() };
        let mut out = Vec::new();
        for page in list {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let words = res.text.map(|t| detection_words(&t, &doc.info.pages[page])).unwrap_or_default();
            let shapes = pdfcraft_forms::detect::page_shapes(cos, page);
            let existing: Vec<[f64; 4]> = doc.form.iter().flat_map(|f| f.widgets.iter()).filter(|w| w.page == Some(page)).map(|w| w.rect).collect();
            for c in pdfcraft_forms::detect::detect(&words, &shapes, &existing, &taken) {
                taken.push(c.name.clone());
                out.push((page, c));
            }
        }
        out
    }

    /// Detect fields and add them, as one undoable step. Returns the new fields' names.
    pub fn auto_detect_fields(&mut self, id: DocId, pages: &[usize]) -> Result<Vec<String>, EditError> {
        let found = self.detect_fields(id, pages);
        if found.is_empty() {
            return Ok(Vec::new());
        }
        let names = found.iter().map(|(_, c)| c.name.clone()).collect();
        let edits = found
            .into_iter()
            .map(|(page, c)| Edit::AddField {
                page,
                rect: c.rect,
                kind: match c.kind {
                    pdfcraft_forms::detect::Kind::Text => pdfcraft_forms::NewField::Text { multiline: c.rect[3] - c.rect[1] > 30.0 },
                    pdfcraft_forms::detect::Kind::CheckBox => pdfcraft_forms::NewField::CheckBox,
                },
                name: Some(c.name),
            })
            .collect();
        self.apply(id, Edit::Batch { label: "Detect form fields".into(), edits })?;
        Ok(names)
    }
}
