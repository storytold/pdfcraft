//! A document's page actions and timers while it is read (#169): page open/close actions run
//! as the reader moves between pages, `app.setInterval` / `app.setTimeOut` timers fire on the
//! viewer's clock, and what they show or hide (an animation's frames) changes only the view.
//! Everything runs in the document's script realm (#5699), shared with its buttons.

use std::collections::BTreeMap;

use pdfcraft_js::{Outcome, Request};

use crate::js::{JsOutput, Runtime, calculation_scripts, field_state};
use crate::{DocId, Document, EditError, Session};

/// Most timers one call to [`Session::run_timers`] runs: a document asking for more at once
/// falls behind instead of holding up the viewer.
pub const MAX_TIMER_RUNS: usize = 16;

/// A timer of a document's scripts, on the caller's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Due {
    id: u32,
    period_ms: u32,
    repeat: bool,
    /// When it falls due, in milliseconds.
    at: f64,
}

/// How a document's page actions and timers show it while it is read. Nothing here is part of
/// the document: saving, printing and exporting never see it.
#[derive(Default)]
pub(crate) struct LiveView {
    /// The page whose open actions ran last.
    page: Option<usize>,
    timers: Vec<Due>,
    /// The realm the timers were set in: a fresh realm (an edit's reset, Revert, a full
    /// save) starts without them.
    timers_of: Option<std::sync::Weak<std::sync::Mutex<pdfcraft_js::session::ScriptSession>>>,
    /// `display` page actions and timers gave fields, by name, over the document's own flags.
    shown: BTreeMap<String, i32>,
    /// Icons page actions and timers gave push buttons (`buttonSetIcon`), by name: the icon's
    /// form object (`None`: no icon), over the button's own.
    icons: BTreeMap<String, Option<u64>>,
    /// The caller's clock when it last looked, in milliseconds: a timer a button or the
    /// console sets counts from here.
    clock: f64,
}

impl LiveView {
    /// The timers, unless the realm that set them was replaced since.
    fn timers(&mut self, runtime: &Runtime) -> &mut Vec<Due> {
        if self.timers_of.as_ref().and_then(std::sync::Weak::upgrade).is_none_or(|r| !std::sync::Arc::ptr_eq(&r, runtime)) {
            self.timers.clear();
            self.timers_of = None;
        }
        &mut self.timers
    }

    /// Keep the timers realm `runtime` reported after a run: new ones count from the clock,
    /// cleared ones go.
    fn schedule(&mut self, runtime: &Runtime, reported: &[pdfcraft_js::Timer]) {
        let clock = self.clock;
        let timers = self.timers(runtime);
        timers.retain(|d| reported.iter().any(|t| t.id == d.id));
        for t in reported {
            if !timers.iter().any(|d| d.id == t.id) {
                timers.push(Due { id: t.id, period_ms: t.period_ms, repeat: t.repeat, at: clock + f64::from(t.period_ms) });
            }
        }
        self.timers_of = (!self.timers.is_empty()).then(|| std::sync::Arc::downgrade(runtime));
    }
}

/// What running a document's page actions or timers did, for the viewer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptRun {
    /// Alerts, console output, requests and errors, as from any script.
    pub output: JsOutput,
    /// Pages showing a field the scripts showed, hid or gave another icon: draw them again.
    pub pages: Vec<usize>,
    /// The scripts changed the document (a field's value, or a reset): read it again.
    pub edited: bool,
    /// How many timers ran.
    pub ran: usize,
    /// Layers were turned on or off (`ocg.state`): draw every page again.
    pub layers: bool,
}

impl ScriptRun {
    pub fn is_empty(&self) -> bool {
        self.output.is_empty() && self.pages.is_empty() && !self.edited && !self.layers
    }
}

/// The document's layers as scripts see them (`getOCGs`), in the viewer's order.
pub(crate) fn js_layers(info: &pdfcraft_render::DocInfo) -> Vec<pdfcraft_js::Layer> {
    info.layers.iter().map(|l| pdfcraft_js::Layer { name: l.name.clone(), on: l.visible, initial: l.initial }).collect()
}

/// A field as scripts see it: as the document has it, but shown as page actions and timers
/// left it (its `display` and push button icon), so a button stepping an animation steps from
/// the frame on screen.
pub(crate) fn live_state(
    doc: &Document,
    cos: &pdfcraft_cos::Document,
    f: &pdfcraft_forms::Field,
    info: &pdfcraft_js::DocInfo,
) -> pdfcraft_js::FieldState {
    let mut s = field_state(cos, f, info);
    if let Some(d) = doc.live.shown.get(&f.name) {
        s.display = *d;
    }
    if let Some(icon) = doc.live.icons.get(&f.name) {
        s.icon = *icon;
    }
    s
}

/// Whether a field with this `display` shows on screen.
fn on_screen(display: i32) -> bool {
    display == pdfcraft_js::DISPLAY_VISIBLE || display == pdfcraft_js::DISPLAY_NO_PRINT
}

/// A widget's object as the renderer names it, when its number fits.
fn object(w: &pdfcraft_forms::Widget) -> Option<(i32, i32)> {
    Some((i32::try_from(w.obj.num).ok()?, i32::from(w.obj.generation)))
}

/// The viewer's render configuration: the document's, with installed fonts, and the fields as
/// page actions and timers show them. Exports and printing use `doc.config`, without them.
pub(crate) fn screen_config(doc: &Document) -> pdfcraft_render::RenderConfig {
    with_live_view(doc, crate::display_config(&doc.config))
}

/// `config` with the fields as page actions and timers show them.
pub(crate) fn with_live_view(doc: &Document, config: pdfcraft_render::RenderConfig) -> pdfcraft_render::RenderConfig {
    let live = &doc.live;
    let annotations: Vec<(i32, i32, bool)> = doc
        .form
        .iter()
        .filter_map(|f| live.shown.get(&f.name).map(|d| (f, on_screen(*d))))
        .flat_map(|(f, on)| f.widgets.iter().filter_map(move |w| object(w).map(|(n, g)| (n, g, on))))
        .collect();
    let appearances: Vec<(i32, i32, i32, i32)> = doc
        .form
        .iter()
        .filter_map(|f| {
            let id = live.icons.get(&f.name)?;
            // No icon: drawn with an object that isn't a form, so nothing (an empty button).
            let (num, generation) = id.map_or((0, 0), |id| ((id >> 16) as i32, (id & 0xffff) as i32));
            Some(f.widgets.iter().filter_map(move |w| object(w).map(|(n, g)| (n, g, num, generation))))
        })
        .flatten()
        .collect();
    pdfcraft_render::RenderConfig { annotations: std::sync::Arc::new(annotations), appearances: std::sync::Arc::new(appearances), ..config }
}

/// Draw the viewer with how fields show now.
fn use_live_view(doc: &mut Document) {
    doc.renderer = pdfcraft_render::RenderPool::new(doc.display.clone(), crate::render_threads(), screen_config(doc));
}

/// Turn layers on or off as a script set them (`ocg.state`), as the Layers panel does: how the
/// document is shown, not the document. Returns whether any changed.
pub(crate) fn set_layers(doc: &mut Document, layers: &[(usize, bool)]) -> bool {
    let mut changed = false;
    for &(i, on) in layers {
        if let Some(l) = doc.info.layers.get_mut(i)
            && l.visible != on
        {
            l.visible = on;
            changed = true;
        }
    }
    if changed {
        crate::layers::use_layer_choices(doc);
    }
    changed
}

/// Push buttons given another icon, by name: the icon's form object (`None`: no icon).
type IconChanges = Vec<(String, Option<u64>)>;

/// The `display` and icon changes in `o`, against the fields as they were (`states`), taken out
/// of it: shown in the view instead of written into the document.
fn take_view_changes(states: &[pdfcraft_js::FieldState], o: &mut Outcome) -> (Vec<(String, i32)>, IconChanges) {
    let mut shown = Vec::new();
    let mut icons = Vec::new();
    for a in &mut o.changed {
        let b = match a.object_id {
            Some(id) => states.iter().find(|s| s.object_id == Some(id)),
            None => states.iter().find(|s| s.name == a.name),
        };
        let Some(b) = b else { continue };
        if a.display != b.display {
            shown.push((a.name.clone(), a.display));
            a.display = b.display;
        }
        if a.icon_written && a.icon != b.icon {
            icons.push((a.name.clone(), a.icon));
        }
        a.icon = b.icon;
        a.icon_written = false;
    }
    (shown, icons)
}

impl Session {
    /// After any script of document `id` ran (a button, the console, a page action, a timer):
    /// keep the timers its realm has, turn layers as it asked, and let a `display` written
    /// into the document win over how page actions and timers showed that field.
    pub(crate) fn after_script(&mut self, id: DocId, o: &Outcome, written: &[String]) {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return };
        let runtime = doc.js_runtime.clone();
        doc.live.schedule(&runtime, &o.timers);
        let before = doc.live.shown.len();
        let icons = doc.live.icons.len();
        doc.live.shown.retain(|name, _| !written.contains(name));
        doc.live.icons.retain(|name, _| !written.contains(name));
        if doc.live.shown.len() != before || doc.live.icons.len() != icons {
            use_live_view(doc);
        }
        set_layers(doc, &o.layers);
    }

    /// Run `script` (or timer `timer`) of document `id` in its realm, as a page action or timer
    /// runs: with the fields as the view shows them. Returns the fields as the script saw them.
    fn run_live(
        &mut self,
        id: DocId,
        script: &str,
        event: pdfcraft_js::Event,
        timer: Option<u32>,
    ) -> Result<(std::sync::Arc<Vec<pdfcraft_js::FieldState>>, Outcome), EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = doc.editor.as_ref().map(|e| e.cos.clone()).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let mut runner = crate::js::JsRunner::new(&cos, &doc.name, self.field_highlight);
        runner.doc.zoom = doc.js_zoom;
        runner.doc.page = doc.live.page.unwrap_or(doc.js_page);
        runner.doc.layers = js_layers(&doc.info);
        let fields = pdfcraft_forms::fields(&cos);
        let states: Vec<_> = fields.iter().map(|f| live_state(doc, &cos, f, &runner.doc)).collect();
        let states = std::sync::Arc::new(states);
        let job = pdfcraft_js::session::Job {
            script: script.into(),
            event,
            doc: runner.doc,
            fields: states.clone(),
            doc_scripts: runner.doc_scripts,
            calculations: calculation_scripts(&cos, &fields),
            timer,
        };
        let o = doc.js_runtime.lock().map_err(|_| EditError::Invalid("document script runtime is unavailable".into()))?.run(job, &[]);
        Ok((states, o))
    }

    /// Run one page action or timer of document `id` and apply what it did. Fields it showed,
    /// hid or gave another icon change only how the document is shown; a value it changed, or
    /// a reset, changes the document (one undoable step). A timer runs unasked and again and
    /// again, so what it asks of the viewer (alerts, printing, navigation, links) goes to the
    /// console instead.
    fn live_step(&mut self, id: DocId, script: &str, event: pdfcraft_js::Event, timer: Option<u32>, run: &mut ScriptRun) {
        let (states, mut o) = match self.run_live(id, script, event, timer) {
            Ok(r) => r,
            Err(e) => return run.output.errors.push(e.to_string()),
        };
        let discard = o.execution_refused && !o.realm_untouched;
        let (shown, icons) = take_view_changes(&states, &mut o);
        if timer.is_some() {
            for a in std::mem::take(&mut o.alerts) {
                o.console.push(format!("A timer's alert: {}", a.text));
            }
            for r in std::mem::take(&mut o.requests) {
                if !matches!(r, Request::Reset(_)) {
                    o.console.push(format!("A timer asked for {r:?}; timers may only change fields"));
                } else {
                    o.requests.push(r);
                }
            }
        }
        let refused = o.execution_refused;
        let generation = self.get(id).map(Document::edit_generation);
        let layers = |s: &Session| s.get(id).map(|d| d.info.layers.iter().map(|l| l.visible).collect::<Vec<_>>());
        let layers_before = layers(self);
        match self.apply_script_outcome(id, &states, o, false) {
            Ok(o) => {
                run.output.absorb(&o);
                // Only now: a script whose edits are refused shows nothing it did either.
                if !refused {
                    self.show_view_changes(id, shown, icons, run);
                }
            }
            Err(e) => run.output.errors.push(e.to_string()),
        }
        run.edited |= self.get(id).map(Document::edit_generation) != generation;
        run.layers |= layers(self) != layers_before;
        if discard {
            self.invalidate_javascript(id);
        }
    }

    /// Show the fields a page action or timer of document `id` showed or hid (`shown`) or gave
    /// another icon (`icons`), and add the pages they are on to `run`.
    fn show_view_changes(&mut self, id: DocId, shown: Vec<(String, i32)>, icons: IconChanges, run: &mut ScriptRun) {
        if shown.is_empty() && icons.is_empty() {
            return;
        }
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return };
        for name in shown.iter().map(|(n, _)| n).chain(icons.iter().map(|(n, _)| n)) {
            for p in doc.form.iter().filter(|f| f.name == *name).flat_map(|f| f.widgets.iter().filter_map(|w| w.page)) {
                if !run.pages.contains(&p) {
                    run.pages.push(p);
                }
            }
        }
        doc.live.shown.extend(shown);
        doc.live.icons.extend(icons);
        use_live_view(doc);
    }

    /// Document `id` is read at page `page` (0-based), at `now_ms` on the caller's clock (on
    /// which its timers fall due). When the page changed, the close actions of the page it was
    /// at run, then `page`'s open actions (the page's own `/AA /O`, then each annotation's
    /// page-open and page-visible actions, such as an animation's widget). Cheap when nothing
    /// changed; call it as often as the page can change. With JavaScript off nothing runs.
    pub fn show_page(&mut self, id: DocId, page: usize, now_ms: f64) -> ScriptRun {
        let mut run = ScriptRun::default();
        let js_off = self.js_off;
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return run };
        doc.live.clock = now_ms;
        if js_off || doc.live.page == Some(page) || doc.xfa_template.is_some() {
            return run;
        }
        let before = doc.live.page.replace(page);
        let Some(cos) = doc.editor.as_ref().map(|e| e.cos.clone()) else { return run };
        use pdfcraft_forms::PageEvent;
        let mut scripts: Vec<(&str, pdfcraft_forms::PageScript)> = Vec::new();
        if let Some(p) = before {
            scripts.extend(pdfcraft_forms::page_scripts(&cos, p, PageEvent::Close).into_iter().map(|s| ("Close", s)));
        }
        scripts.extend(pdfcraft_forms::page_scripts(&cos, page, PageEvent::Open).into_iter().map(|s| ("Open", s)));
        for (name, s) in scripts {
            let event = pdfcraft_js::Event { kind: "Page".into(), target: s.field, ..pdfcraft_js::Event::doc(name) };
            self.live_step(id, &s.script, event, None, &mut run);
        }
        run
    }

    /// When document `id`'s next timer falls due, on the clock [`Session::show_page`] and
    /// [`Session::run_timers`] are given; `None` without timers (or with JavaScript off).
    pub fn next_timer(&mut self, id: DocId) -> Option<f64> {
        if self.js_off {
            return None;
        }
        let doc = self.docs.iter_mut().find(|d| d.id == id)?;
        let runtime = doc.js_runtime.clone();
        doc.live.timers(&runtime).iter().map(|d| d.at).min_by(f64::total_cmp)
    }

    /// Run document `id`'s timers that are due at `now_ms`, earliest first, at most
    /// [`MAX_TIMER_RUNS`] of them. An interval that fell behind runs once and then counts its
    /// period from now, rather than catching up run by run.
    pub fn run_timers(&mut self, id: DocId, now_ms: f64) -> ScriptRun {
        self.run_timers_at_most(id, now_ms, MAX_TIMER_RUNS)
    }

    /// [`Session::run_timers`], running at most `max` timers (and never more than
    /// [`MAX_TIMER_RUNS`]). With `max` 0 it only moves the clock on to `now_ms`: the timers still
    /// due run at the next call.
    pub fn run_timers_at_most(&mut self, id: DocId, now_ms: f64, max: usize) -> ScriptRun {
        let mut run = ScriptRun::default();
        if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
            doc.live.clock = now_ms;
        }
        for _ in 0..max.min(MAX_TIMER_RUNS) {
            if self.js_off {
                break;
            }
            let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { break };
            doc.live.clock = now_ms;
            let runtime = doc.js_runtime.clone();
            let timers = doc.live.timers(&runtime);
            let Some(i) = timers.iter().enumerate().filter(|(_, d)| d.at <= now_ms).min_by(|a, b| a.1.at.total_cmp(&b.1.at)).map(|(i, _)| i) else {
                break;
            };
            let due = timers[i];
            if due.repeat {
                let next = due.at + f64::from(due.period_ms);
                timers[i].at = if next <= now_ms { now_ms + f64::from(due.period_ms) } else { next };
            } else {
                timers.remove(i);
            }
            run.ran += 1;
            let event = pdfcraft_js::Event { kind: "App".into(), ..pdfcraft_js::Event::doc("Timer") };
            self.live_step(id, "", event, Some(due.id), &mut run);
        }
        run
    }

    /// The clock of document `id`'s live scripts: the time it was last given (milliseconds).
    pub fn script_clock(&self, id: DocId) -> Option<f64> {
        Some(self.get(id)?.live.clock)
    }
}

impl Document {
    /// Whether the document's page actions and timers show field `name` on screen (`Some`), over
    /// its widgets' own Hidden and NoView flags; `None` when they left it as the document has it.
    /// For the viewer's hit test: a frame they showed takes presses, and one they hid doesn't.
    pub fn shown_by_scripts(&self, name: &str) -> Option<bool> {
        self.live.shown.get(name).map(|d| on_screen(*d))
    }
}

#[cfg(test)]
#[path = "live_scripts_tests.rs"]
mod tests;
