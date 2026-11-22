//! Action Wizard: the actions (PdfCraft's and your own), running one over files in the
//! background, and creating or editing an action's steps. Your actions are remembered.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::docpath::names::{CopyNumbering, unique_file_name};
use craft::host::queue::Priority;
use craft::host::{Files, PickRequest, Picked, WriteIntent};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, Entry, Props as _, RowAlign, Show as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Text};
use craft::ui::form::{Field, FieldGroup};
use craft::ui::layout_components::Row;
use craft::ui::patterns::form_dialogs::{FormDialog, ListManager, ListManagerAction, ListManagerChoice, ListManagerEntry, ListManagerWords};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Env;
use craft::ui::text::TextField;
use egui::Ui;
use pdfcraft_engine::actions::{Action, Step, builtin};

use super::job_count::JobCount;
use crate::PdfCraftApp;
use crate::notices::{self, ProgressKey, ProgressNotice};

/// What a finished run reports: its summary, in the UI's language.
pub struct ActionOutcome {
    /// The summary the notice shows.
    pub message: String,
}

/// The queue of Action Wizard runs: one at a time, each answering with its summary.
pub(crate) type ActionJobs = craft::host::queue::JobQueue<(), ActionOutcome>;

/// An action being created or edited, and the step the Add Step choice offers.
#[derive(Clone, Default, PartialEq)]
struct ActionDraft {
    name: String,
    description: String,
    steps: Vec<Step>,
    add: usize,
}

/// The wizard's description of an action: its description, then its numbered steps, one per line.
fn description_text(a: &Action) -> String {
    let mut lines = Vec::new();
    if !a.description.is_empty() {
        lines.push(if a.builtin { tl!(&a.description).to_string() } else { a.description.clone() });
    }
    for (i, s) in a.steps.iter().enumerate() {
        let arg = s.arg().filter(|x| !x.is_empty()).map(|x| format!(": {x}")).unwrap_or_default();
        lines.push(format!("{}. {}{arg}", i + 1, tl!(s.label())));
    }
    lines.join("\n")
}

/// The name an action shows under: built-in names are translated.
fn shown_name(a: &Action) -> String {
    if a.builtin { tl!(&a.name).to_string() } else { a.name.clone() }
}

/// The Action Wizard: the list of actions with Start, New, Edit and Delete beside it.
pub(crate) struct ActionWizardDialog {
    id: DialogId,
    entries: Vec<ListManagerEntry>,
    select: Option<String>,
}

impl StackDialog for ActionWizardDialog {
    type Result = ListManagerChoice;

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<ListManagerChoice>> {
        let words = Env::of(ui).words::<ListManagerWords>();
        let mut start = ListManagerAction::run(&words);
        start.label = tl_ctx!("action wizard", "Start").into();
        let actions = [start, ListManagerAction::create(&words), ListManagerAction::edit(&words).kind(1), ListManagerAction::delete(&words).kind(1)];
        let mut manager = ListManager::new(tl!("Action Wizard"), tl!("Action name"), self.entries.as_slice(), actions.as_slice());
        if let Some(name) = &self.select {
            manager = manager.name(name.clone());
        }
        let shown = manager.show(ui);
        match (shown.choice, shown.closed) {
            (Some(choice), _) => Some(DialogResult::Ok(choice)),
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }
}

/// The custom-action editor: name, description and ordered steps. Save stays disabled until the name is
/// unique and non-empty and at least one step exists.
pub(crate) struct ActionEditorDialog {
    id: DialogId,
    draft: ActionDraft,
    /// The name the action had, when it is being edited.
    original: Option<String>,
    /// The names no new name may equal: the built-in actions' and the other custom actions'.
    taken: Vec<String>,
}

impl StackDialog for ActionEditorDialog {
    type Result = (Option<String>, Action);

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<(Option<String>, Action)>> {
        let name = self.draft.name.trim().to_string();
        let taken = self.taken.contains(&name);
        let ok = !name.is_empty() && !taken && !self.draft.steps.is_empty();
        let title = if self.original.is_some() { tl!("Edit Action") } else { tl!("New Action") };
        let mut form = FormDialog::new(title, Bind::bound(&mut self.draft)).ok_label(tl!("Save")).ok_enabled(ok);
        if taken {
            form = form.ok_reason(tl!("An action with this name already exists."));
        }
        let shown = form.show(ui, |ui, _, d| {
            Field::new(tl!("Action name:")).show(ui, |ui, _| {
                TextField::new(tl!("Action name:"), Bind::bound(&mut d.name)).show(ui);
            });
            Field::new(tl!("Description:")).show(ui, |ui, _| {
                TextField::new(tl!("Description:"), Bind::bound(&mut d.description)).show(ui);
            });
            FieldGroup::new(tl!("Steps")).show(ui, |ui, _| {
                if d.steps.is_empty() {
                    Text::new(tl!("Add the steps this action runs, in order.")).show(ui);
                }
                let mut remove = None;
                for (i, s) in d.steps.iter_mut().enumerate() {
                    let plain = s.arg_mut().is_none();
                    let spread = if plain { RowAlign::SpaceBetween } else { RowAlign::Start };
                    Row::new().fill_width().align(spread).show(ui, |ui, _| {
                        let label = format!("{}. {}", i + 1, tl!(s.label()));
                        Text::new(label.clone()).show(ui);
                        if let Some(arg) = s.arg_mut() {
                            TextField::new(label, Bind::bound(arg)).show(ui);
                        }
                        if Button::new(tl!("Remove")).show(ui).clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    d.steps.remove(i);
                }
                let all = Step::all();
                let last = all.len().saturating_sub(1);
                let choices: Vec<Entry<usize>> = all.iter().enumerate().map(|(i, s)| Entry::new(i, tl!(s.label()))).collect();
                Row::new().show(ui, |ui, _| {
                    if let Some(pick) = Select::new(tl!("Step to add"), Bind::given(d.add.min(last)), &choices).show(ui).value().copied() {
                        d.add = pick;
                    }
                    if Button::new(tl!("Add Step")).show(ui).clicked()
                        && let Some(step) = all.get(d.add.min(last))
                    {
                        d.steps.push(step.clone());
                    }
                });
            });
            if taken {
                Text::new(tl!("An action with this name already exists.")).shade(Shade::Muted).show(ui);
            }
            true
        });
        match (shown.accepted, shown.closed) {
            (Some(d), _) => {
                let action = Action { name: d.name.trim().to_string(), description: d.description, steps: d.steps, builtin: false };
                Some(DialogResult::Ok((self.original.clone(), action)))
            }
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }
}

/// Opens the wizard listing every action, with the entry named `select` (as shown) chosen.
fn open_list(app: &mut PdfCraftApp, select: Option<String>) {
    let entries = app
        .all_actions()
        .iter()
        .map(|a| ListManagerEntry::new(a.name.clone(), shown_name(a)).description(description_text(a)).kind(u32::from(!a.builtin)))
        .collect();
    crate::dialogs::host::open(app, ActionWizardDialog { id: dialog_id("action.wizard"), entries, select });
}

/// Opens the Action Wizard with the built-in or custom action named `preselect` (its untranslated name) chosen.
pub(crate) fn open_wizard(app: &mut PdfCraftApp, _ctx: &egui::Context, preselect: Option<&str>) {
    let select = preselect.and_then(|n| app.all_actions().iter().find(|a| a.name == n).map(shown_name));
    open_list(app, select);
}

/// Whether the Action Wizard is on the dialog host.
pub(crate) fn wizard_open(app: &PdfCraftApp) -> bool {
    app.dialogs.is_open::<ActionWizardDialog>()
}

/// Opens the editor over `draft`; `original` is the name the action had when it is being edited.
fn open_editor(app: &mut PdfCraftApp, original: Option<String>, draft: ActionDraft) {
    let mut taken: Vec<String> = builtin().into_iter().map(|a| a.name).collect();
    taken.extend(app.custom_actions.iter().filter(|c| original.as_deref() != Some(c.name.as_str())).map(|c| c.name.clone()));
    crate::dialogs::host::open(app, ActionEditorDialog { id: dialog_id("action.editor"), draft, original, taken });
}

/// Takes what the wizard and the editor answered this pass.
pub(crate) fn pass(app: &mut PdfCraftApp) {
    if let Some(DialogResult::Ok(choice)) = app.dialogs.take::<ActionWizardDialog>() {
        let entry = choice.entry.clone().unwrap_or_default();
        match choice.action.as_str() {
            "run" => app.start_selected_action(&entry),
            "create" => open_editor(app, None, ActionDraft { name: choice.name, ..Default::default() }),
            "edit" => {
                if let Some(a) = app.custom_actions.iter().find(|a| a.name == entry).cloned() {
                    let draft = ActionDraft { name: a.name.clone(), description: a.description, steps: a.steps, add: 0 };
                    open_editor(app, Some(a.name), draft);
                }
            }
            "delete" => {
                app.custom_actions.retain(|a| a.name != entry);
                open_list(app, None);
            }
            _ => {}
        }
    }
    match app.dialogs.take::<ActionEditorDialog>() {
        Some(DialogResult::Ok((original, action))) => {
            let name = action.name.clone();
            match original.and_then(|o| app.custom_actions.iter().position(|c| c.name == o)) {
                Some(i) => app.custom_actions[i] = action,
                None => app.custom_actions.push(action),
            }
            open_list(app, Some(name));
        }
        // Any answer but OK leaves the editor and returns to the list, as Cancel does.
        Some(_) => open_list(app, None),
        None => {}
    }
}

/// Keep each result, including when input basenames repeat or an earlier run already used the name.
/// The search is bounded and never falls back to an occupied destination.
fn write_result(files: &dyn Files, dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let taken = |n: &str| files.read(&dir.join(n)).is_ok();
    let unique = unique_file_name(name, CopyNumbering::Parenthesized, taken);
    if taken(&unique) {
        return Err(crate::i18n::fmt(tl!("No free file name for {name}"), &[("name", name)]));
    }
    files.write(&dir.join(&unique), bytes, WriteIntent::SaveAs).map(|_| ()).map_err(|e| e.to_string())
}

impl PdfCraftApp {
    /// Built-in actions, then the user's.
    pub fn all_actions(&self) -> Vec<Action> {
        let mut v = builtin();
        v.extend(self.custom_actions.iter().cloned());
        v
    }

    /// Run `action` on files, writing the results into a folder the user picks, in the background.
    pub fn run_action_on(&mut self, action: Action, files: Vec<(String, Vec<u8>)>) {
        if self.action_run.is_some() {
            self.notify_tr("An action is already running");
            return;
        }
        // The web has no folder to pick: the results download.
        #[cfg(target_arch = "wasm32")]
        self.run_action_into(action, files, PathBuf::new());
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_first(PickRequest::folder("action.results").title(tl!("Choose a folder for the results")), None, move |app, _, dir| {
            app.run_action_into(action, files, dir)
        });
    }

    /// [`Self::run_action_on`] once the folder for the results is known (empty on the web).
    fn run_action_into(&mut self, action: Action, files: Vec<(String, Vec<u8>)>, dir: PathBuf) {
        // Another run may have started while the folder picker was open.
        if self.action_run.is_some() {
            self.notify_tr("An action is already running");
            return;
        }

        let count = Arc::new(JobCount::default());
        let (job_count, wake) = (count.clone(), self.wake.clone());
        let host = self.host.clone();

        // The summary is written on a worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        // Each file runs in its own session; it keeps this one's JavaScript preference.
        let javascript = self.session.javascript();
        let total = files.len();
        self.action_jobs.submit((), Priority::VISIBLE, move |_| {
            crate::i18n::set_current(lang);

            let (mut ok, mut failed) = (0, Vec::new());
            for (i, (name, bytes)) in files.into_iter().enumerate() {
                job_count.set(i, total);
                wake.wake();
                let r = pdfcraft_engine::actions::run_on(&action, &name, Arc::new(bytes), javascript, |_, _| {})
                    .and_then(|r| write_result(host.files(), &dir, &name, &r.bytes));
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push(format!("{name}: {e}")),
                }
            }
            job_count.set(total, total);
            let done =
                if ok == 1 { crate::i18n::fmt(tl!("1 file done"), &[]) } else { crate::i18n::fmt(tl!("{n} files done"), &[("n", &ok.to_string())]) };
            let mut msg = format!("{}: {done}", action.name);
            if !failed.is_empty() {
                msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
            }
            ActionOutcome { message: msg }
        });
        self.action_run = Some(count);
    }

    /// Show the running action's progress, and its summary when done.
    pub(crate) fn poll_action(&mut self) {
        // Without lanes the owner runs the job, one per pass.
        #[cfg(target_arch = "wasm32")]
        self.action_jobs.run_inline(1);
        let Some(count) = self.action_run.clone() else { return };
        if let Some(finished) = self.action_jobs.try_recv() {
            self.action_run = None;
            notices::end_progress(self, ProgressKey::Action);
            self.notify(finished.result.message);
            return;
        }
        let Some((done, total)) = count.take_change() else { return };
        let total = total.max(1);
        let shown = (done + 1).min(total);
        let label = crate::i18n::fmt(tl!("Running action… file {d} of {t}"), &[("d", &shown.to_string()), ("t", &total.to_string())]);
        let fraction = done as f32 / total as f32;
        notices::set_progress(self, ProgressKey::Action, ProgressNotice { label, fraction, cancellable: false });
    }

    /// Start: ask for the files, then run the action named `name` on them.
    pub(crate) fn start_selected_action(&mut self, name: &str) {
        let Some(action) = self.all_actions().into_iter().find(|a| a.name == name) else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let title = crate::i18n::fmt(tl!("Files for {name}"), &[("name", &action.name)]);
            let request = PickRequest::open_many("action.files").title(title).filter("PDF", &["pdf"]);
            self.pick_file(request, None, move |app, _, picked| {
                let Picked::Paths(paths) = picked else { return };
                let mut files = Vec::new();
                for p in paths {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    match app.host.files().read(&p) {
                        Ok(b) => files.push((name, b)),
                        Err(e) => return app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
                    }
                }
                if !files.is_empty() {
                    // Asks for the results folder: a second picker, now that this one has closed.
                    app.run_action_on(action, files);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The open document stands in for a file picker.
            if let Some(doc) = self.active_ids().and_then(|(_, id)| self.session.get(id)) {
                let files = vec![(doc.name.clone(), doc.bytes.to_vec())];
                self.run_action_on(action, files);
            }
        }
    }
}

/// Saved actions as JSON (`[{name, description, steps: [[id, arg], …]}]`).
pub fn encode(actions: &[Action]) -> serde_json::Value {
    serde_json::Value::Array(
        actions
            .iter()
            .map(|a| {
                let steps: Vec<serde_json::Value> = a.steps.iter().map(|s| serde_json::json!([s.id(), s.arg().unwrap_or("")])).collect();
                serde_json::json!({ "name": a.name, "description": a.description, "steps": steps })
            })
            .collect(),
    )
}

pub fn decode(v: &serde_json::Value) -> Vec<Action> {
    v.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    let name = a["name"].as_str()?.to_string();
                    let steps = a["steps"].as_array()?.iter().filter_map(|s| Step::from_id(s[0].as_str()?, s[1].as_str().unwrap_or(""))).collect();
                    Some(Action { name, description: a["description"].as_str().unwrap_or("").to_string(), steps, builtin: false })
                })
                .collect()
        })
        .unwrap_or_default()
}
