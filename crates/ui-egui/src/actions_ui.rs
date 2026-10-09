//! Action Wizard: the actions (PdfCraft's and your own), running one over files in the
//! background, and creating or editing an action's steps. Your actions are remembered.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::actions::{Action, Step, builtin};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The Action Wizard's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wizard {
    /// The selected action, by name.
    pub selected: Option<String>,
    /// The action being created or edited (and the name it had).
    pub editing: Option<(Option<String>, Action)>,
    /// The step the "Add" menu adds.
    pub add: usize,
}

/// Progress of a run: files done, total, and the summary once finished.
#[derive(Default)]
pub struct RunProgress {
    pub done: usize,
    pub total: usize,
    pub message: Option<String>,
}

impl PdfCraftApp {
    /// Built-in actions, then the user's.
    pub fn all_actions(&self) -> Vec<Action> {
        let mut v = builtin();
        v.extend(self.custom_actions.iter().cloned());
        v
    }

    /// Run `action` on files, writing the results into a folder the user picks (the export
    /// folder override in tests), in the background.
    pub fn run_action_on(&mut self, action: Action, files: Vec<(String, Vec<u8>)>) {
        if self.action_run.is_some() {
            self.notify_tr("An action is already running");
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        match self.export_dir_override.clone() {
            Some(d) => self.run_action_into(action, files, d.into()),
            None => {
                let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose a folder for the results").to_string());
                self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| app.run_action_into(action, files, dir));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.run_action_into(action, files);
    }

    /// [`Self::run_action_on`] once the folder for the results is known.
    fn run_action_into(&mut self, action: Action, files: Vec<(String, Vec<u8>)>, #[cfg(not(target_arch = "wasm32"))] dir: std::path::PathBuf) {
        // Another run may have started while the folder picker was open.
        if self.action_run.is_some() {
            self.notify_tr("An action is already running");
            return;
        }
        let progress = Arc::new(Mutex::new(RunProgress { total: files.len(), ..Default::default() }));
        let p = progress.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let (mut ok, mut failed) = (0, Vec::new());
            for (i, (name, bytes)) in files.into_iter().enumerate() {
                if let Ok(mut s) = p.lock() {
                    s.done = i;
                }
                let r = pdfcraft_engine::actions::run_on(&action, &name, Arc::new(bytes), |_, _| {}).and_then(|r| {
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::editing::write_atomically(&dir.join(&name).to_string_lossy(), &r.bytes).map_err(|e| e.to_string())?;
                    #[cfg(target_arch = "wasm32")]
                    crate::editing::download(&name, &r.bytes)?;
                    Ok(())
                });
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push(format!("{name}: {}", e)),
                }
            }
            let done =
                if ok == 1 { crate::i18n::fmt(tl!("1 file done"), &[]) } else { crate::i18n::fmt(tl!("{n} files done"), &[("n", &ok.to_string())]) };
            let mut msg = format!("{}: {done}", action.name);
            if !failed.is_empty() {
                msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
            }
            if let Ok(mut s) = p.lock() {
                s.done = s.total;
                s.message = Some(msg);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-action".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.action_run = Some(progress);
        self.poll_action();
    }

    /// Show the running action's progress, and its summary when done.
    pub(crate) fn poll_action(&mut self) {
        let Some(p) = self.action_run.clone() else { return };
        let state = p.lock().ok().map(|mut s| s.message.take().ok_or((s.done, s.total)));
        match state {
            Some(Ok(m)) => {
                self.action_run = None;
                self.notify(m);
            }
            Some(Err((done, total))) => {
                let m = crate::i18n::fmt(
                    tl!("Running action… file {d} of {t}"),
                    &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                );
                if self.toast.as_ref().is_none_or(|t| t.0 != m) {
                    self.notify(m);
                }
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
            }
            None => self.action_run = None,
        }
    }

    /// Start: ask for the files, then run the selected action on them.
    fn start_selected_action(&mut self) {
        let Some(action) = self.wizard.selected.as_ref().and_then(|n| self.all_actions().into_iter().find(|a| &a.name == n)) else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let title = crate::i18n::fmt(tl!("Files for {name}"), &[("name", &action.name)]);
            let run = move |app: &mut Self, paths: Vec<std::path::PathBuf>| {
                let mut files = Vec::new();
                for p in paths {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    match std::fs::read(&p) {
                        Ok(b) => files.push((name, b)),
                        Err(e) => return app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
                    }
                }
                if !files.is_empty() {
                    // Asks for the results folder: a second picker, now that this one has closed.
                    app.run_action_on(action, files);
                }
            };
            match self.action_files_override.clone() {
                Some(p) => run(self, p.into_iter().map(std::path::PathBuf::from).collect()),
                None => {
                    let dialog = rfd::AsyncFileDialog::new().set_title(title).add_filter("PDF", &["pdf"]);
                    self.ask(crate::pickers::Ask::Files(dialog), None, run);
                }
            }
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

/// The Action Wizard dialog. Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    if app.wizard.editing.is_some() {
        return edit_body(ui, app, t);
    }
    ui.label(egui::RichText::new(tl!("Action Wizard")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let actions = app.all_actions();
    if app.wizard.selected.as_ref().is_none_or(|s| !actions.iter().any(|a| &a.name == s)) {
        app.wizard.selected = actions.first().map(|a| a.name.clone());
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(220.0);
            egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(208.0);
                ui.set_min_height(260.0);
                for a in &actions {
                    let on = app.wizard.selected.as_deref() == Some(a.name.as_str());
                    let name = if a.builtin { tl!(&a.name) } else { &a.name };
                    if ui.selectable_label(on, name).clicked() {
                        app.wizard.selected = Some(a.name.clone());
                    }
                }
            });
        });
        ui.vertical(|ui| {
            let Some(a) = actions.iter().find(|a| app.wizard.selected.as_deref() == Some(a.name.as_str())) else { return };
            let name = if a.builtin { tl!(&a.name) } else { &a.name };
            ui.label(egui::RichText::new(name).font(theme::semibold(15.0)));
            if !a.description.is_empty() {
                let description = if a.builtin { tl!(&a.description) } else { &a.description };
                ui.label(egui::RichText::new(description).color(t.text_muted));
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("Steps")).font(theme::semibold(13.0)));
            for (i, s) in a.steps.iter().enumerate() {
                let arg = s.arg().filter(|x| !x.is_empty()).map(|x| format!(": {x}")).unwrap_or_default();
                ui.label(format!("{}. {}{arg}", i + 1, tl!(s.label())));
            }
        });
    });
    ui.add_space(10.0);
    let selected_custom = app.wizard.selected.as_ref().and_then(|n| app.custom_actions.iter().position(|a| &a.name == n));
    let mut close = false;
    ui.horizontal(|ui| {
        if ui.button(tl!("New Action…")).clicked() {
            app.wizard.editing = Some((None, Action { name: String::new(), description: String::new(), steps: Vec::new(), builtin: false }));
        }
        if let Some(i) = selected_custom {
            if ui.button(tl!("Edit…")).clicked() {
                let a = app.custom_actions[i].clone();
                app.wizard.editing = Some((Some(a.name.clone()), a));
            }
            if ui.button(tl!("Delete")).clicked() {
                app.custom_actions.remove(i);
                app.wizard.selected = None;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl_ctx!("action wizard", "Start"), true).clicked() {
                close = true;
                app.start_selected_action();
            }
            if widgets::pill_button(ui, tl!("Close"), false).clicked() {
                close = true;
            }
        });
    });
    close
}

fn edit_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let Some((original, a)) = app.wizard.editing.as_mut() else { return false };
    ui.label(egui::RichText::new(if original.is_some() { tl!("Edit Action") } else { tl!("New Action") }).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("action-edit").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Action name:"));
        ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(320.0).id_salt("action-name"));
        ui.end_row();
        ui.label(tl!("Description:"));
        ui.add(egui::TextEdit::singleline(&mut a.description).desired_width(320.0).id_salt("action-description"));
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Steps")).font(theme::semibold(13.0)));
    let mut remove = None;
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if a.steps.is_empty() {
            ui.label(egui::RichText::new(tl!("Add the steps this action runs, in order.")).color(t.text_muted));
        }
        for (i, s) in a.steps.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{}. {}", i + 1, tl!(s.label())));
                    if let Some(arg) = s.arg_mut() {
                        ui.add(egui::TextEdit::singleline(arg).desired_width(220.0));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(tl!("Remove")).clicked() {
                            remove = Some(i);
                        }
                    });
                });
            });
        }
    });
    if let Some(i) = remove {
        a.steps.remove(i);
    }
    let all = Step::all();
    let add = &mut app.wizard.add;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("action-add-step").selected_text(tl!(all[(*add).min(all.len() - 1)].label())).width(240.0).show_ui(ui, |ui| {
            for (i, s) in all.iter().enumerate() {
                ui.selectable_value(add, i, tl!(s.label()));
            }
        });
        if ui.button(tl!("Add Step")).clicked() {
            a.steps.push(all[(*add).min(all.len() - 1)].clone());
        }
    });
    ui.add_space(10.0);
    let name = a.name.trim().to_string();
    let taken =
        builtin().iter().any(|b| b.name == name) || app.custom_actions.iter().any(|c| c.name == name && original.as_deref() != Some(name.as_str()));
    if taken {
        ui.label(egui::RichText::new(tl!("An action with this name already exists.")).small().color(t.text_muted));
    }
    let (mut done, mut cancel) = (false, false);
    let ok = !name.is_empty() && !taken && !a.steps.is_empty();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("Save"), true)).inner.clicked() {
                done = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        app.wizard.editing = None;
    }
    if done && let Some((original, mut a)) = app.wizard.editing.take() {
        a.name = name.clone();
        match original.and_then(|o| app.custom_actions.iter().position(|c| c.name == o)) {
            Some(i) => app.custom_actions[i] = a,
            None => app.custom_actions.push(a),
        }
        app.wizard.selected = Some(name);
    }
    false
}
