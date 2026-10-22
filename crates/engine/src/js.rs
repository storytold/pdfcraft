//! Acrobat JavaScript in documents: field scripts run during form edits ([`JsRunner`], wired
//! into `forms` through its [`pdfcraft_forms::Scripts`] hook), and button actions and console
//! input run on request ([`Session::run_javascript`]). What scripts ask of the viewer (alerts,
//! printing, navigation, links) is kept for the front end in [`JsOutput`].
//!
//! Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript is [`Session::set_javascript`]; with it
//! off, scripts don't run and AF calls keep working natively.

use pdfcraft_forms::{Field, FieldChange, FieldEvent, FieldKind, FieldProps, ScriptResult, Scripts};
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
    pub fn new(cos: &pdfcraft_cos::Document, file_name: &str) -> JsRunner {
        let doc = pdfcraft_js::DocInfo { file_name: file_name.to_string(), num_pages: pdfcraft_model::pages(cos).len(), page: 0, info: info(cos) };
        JsRunner {
            doc,
            doc_scripts: pdfcraft_forms::document_scripts(cos),
            output: JsOutput::default(),
            cos: cos.clone(),
            displays: std::collections::HashMap::new(),
        }
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
        let changes = changes(&states, &o.changed);
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

/// A page's words in reading order, their boxes mapped from view space by `map` (and ordered
/// again, so that [x0, y0, x1, y1] holds whatever way `map` turns them).
pub(crate) fn words_with(text: &pdfcraft_render::PageText, map: impl Fn(f32, f32) -> [f32; 2]) -> Vec<(String, [f32; 4])> {
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

#[cfg(test)]
mod review_tests {
    use super::*;

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
