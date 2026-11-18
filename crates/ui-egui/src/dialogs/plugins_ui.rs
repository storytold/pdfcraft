//! Plug-ins (RFC-0046): the Plug-ins menu, the Plug-ins window (what is installed, installing from
//! a file, a GitHub repository or a catalog behind a confirmation, updating and removing) and the
//! form a plug-in command's parameters are entered in. The work itself is the automation tools'
//! (`plugin_list`, `plugin_install`, `plugin_run`…), which the desktop app wires in with a
//! [`PluginBackend`]; the web build has none and shows no Plug-ins menu. Nothing here is drawn on
//! the pages: everything is in the window, and a run acts like an Undo step does afterwards.

use std::sync::mpsc;

use craft_menu::{CommandRef, Item, Node};
use pdfcraft_engine::Session;
use serde_json::{Map, Value, json};

use crate::{PdfCraftApp, widgets};

/// Menu ids: the Plug-ins window, and one plug-in command (`{"id", "command"}`).
pub(crate) const MANAGE: &str = "plugins.manage";
pub(crate) const RUN: &str = "plugins.run";

/// A worker's job: what to do with a tool's answer on the UI thread.
type JobResult = Result<Value, String>;

/// The plug-in tools, as the desktop app provides them: automation's `plugin_*` tools over this
/// window's session.
pub trait PluginBackend {
    /// Run plug-in tool `tool` (`plugin_list`, `plugin_install`, `plugin_remove`, `plugin_run`,
    /// `plugin_catalog_add`…) with `args` over `session`, and give its JSON answer.
    fn call(&mut self, session: &mut Session, tool: &str, args: &Value) -> Result<Value, String>;

    /// The same tool as a job for a worker thread, for what downloads and needs no document:
    /// `plugin_install` with `review: true` (it keeps the reviewed bundle for the install that
    /// follows), `plugin_update` and `plugin_catalog_list`.
    fn job(&mut self, tool: &str, args: Value) -> Box<dyn FnOnce() -> JobResult + Send>;
}

/// What a worker is doing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Work {
    Review,
    Update,
    Catalog,
}

/// A bundle shown for confirmation: the review and the `plugin_install` arguments that install it.
struct Review {
    info: Value,
    request: Value,
}

/// A plug-in command's parameter form.
struct Form {
    id: String,
    command: String,
    title: String,
    params: Vec<Value>,
    values: Map<String, Value>,
}

/// The Plug-ins menu's and window's state.
#[derive(Default)]
pub(crate) struct Plugins {
    backend: Option<Box<dyn PluginBackend>>,
    pub(crate) open: bool,
    /// What `plugin_list` said last.
    listing: Value,
    repo: String,
    catalog: String,
    packages: Vec<Value>,
    review: Option<Review>,
    job: Option<(Work, mpsc::Receiver<JobResult>)>,
    /// The last outcome to show in the window: (is an error, text).
    status: Option<(bool, String)>,
    form: Option<Form>,
}

impl Plugins {
    /// Whether this build runs plug-ins (the desktop app wired a backend in).
    pub(crate) fn available(&self) -> bool {
        self.backend.is_some()
    }

    fn installed(&self) -> &[Value] {
        self.listing["plugins"].as_array().map_or(&[], Vec::as_slice)
    }

    fn plugin(&self, id: &str) -> Option<&Value> {
        self.installed().iter().find(|p| p["id"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(id)))
    }

    fn start(&mut self, work: Work, tool: &str, args: Value, ctx: &egui::Context) {
        let Some(backend) = self.backend.as_mut() else { return };
        let run = backend.job(tool, args);
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let task = move || {
            let _ = sender.send(run());
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = std::thread::Builder::new().name("plugins".into()).spawn(task) {
            self.status = Some((true, e.to_string()));
            return;
        }
        #[cfg(target_arch = "wasm32")]
        task();
        self.status = None;
        self.job = Some((work, receiver));
    }

    /// A finished job's answer, if one came.
    fn poll(&mut self) {
        let Some((work, receiver)) = &self.job else { return };
        let work = *work;
        let answer = match receiver.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(tl!("The plug-in download stopped unexpectedly.").into()),
        };
        self.job = None;
        match (work, answer) {
            (_, Err(e)) => self.status = Some((true, e)),
            (Work::Review, Ok(info)) => {
                let request = info.get("request").cloned().unwrap_or(Value::Null);
                self.review = Some(Review { info, request });
            }
            (Work::Update, Ok(v)) if v["update"].is_null() => {
                self.status = Some((false, crate::i18n::fmt(tl!("{name} is up to date."), &[("name", v["id"].as_str().unwrap_or_default())])));
            }
            (Work::Update, Ok(v)) => self.review = Some(Review { info: v["review"].clone(), request: v["install"].clone() }),
            (Work::Catalog, Ok(v)) => {
                self.packages = v["packages"].as_array().cloned().unwrap_or_default();
                if let Some(w) = v["warnings"].as_array().and_then(|w| w.first()).and_then(Value::as_str) {
                    self.status = Some((true, w.to_string()));
                }
            }
        }
    }
}

/// A parameter's starting value: its default, or an empty one of its kind.
fn default_value(p: &Value) -> Value {
    if let Some(d) = p.get("default").filter(|d| !d.is_null()) {
        return d.clone();
    }
    match p["kind"].as_str().unwrap_or("text") {
        "number" | "slider" => json!(p["min"].as_f64().unwrap_or(0.0)),
        "boolean" => json!(false),
        "select" | "list" => p["options"].get(0).cloned().unwrap_or(Value::Null),
        "color" => json!("#000000"),
        _ => json!(""),
    }
}

/// A plug-in command's name in the menu: the plug-in's, and the command's when it has several.
fn command_label(plugin: &Value, command: &Value) -> String {
    let name = plugin["name"].as_str().unwrap_or_default();
    let several = plugin["commands"].as_array().is_some_and(|c| c.len() > 1);
    match command["label"].as_str() {
        Some(label) if several => format!("{name}: {label}"),
        _ => name.to_string(),
    }
}

impl PdfCraftApp {
    /// Run plug-ins through `backend` (the desktop app's automation tools), and read the installed
    /// ones for the Plug-ins menu.
    pub fn set_plugin_backend(&mut self, backend: Box<dyn PluginBackend>) {
        self.plugins.backend = Some(backend);
        self.refresh_plugins();
    }

    /// Call plug-in tool `tool` over this window's session.
    fn plugin_tool(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let Some(backend) = self.plugins.backend.as_mut() else { return Err(tl!("Plug-ins aren't available here.").into()) };
        backend.call(&mut self.session, tool, args)
    }

    /// Read the installed plug-ins again.
    pub(crate) fn refresh_plugins(&mut self) {
        match self.plugin_tool("plugin_list", &json!({})) {
            Ok(listing) => self.plugins.listing = listing,
            Err(e) => self.plugins.status = Some((true, e)),
        }
    }

    /// The Plug-ins menu's items: every installed plug-in's commands, then the Plug-ins window.
    /// `None` in a build without plug-ins (the web).
    pub(crate) fn plugins_menu(&self) -> Option<Vec<Node>> {
        if !self.plugins.available() {
            return None;
        }
        let mut nodes: Vec<Node> = Vec::new();
        for plugin in self.plugins.installed() {
            let Some(id) = plugin["id"].as_str() else { continue };
            for command in plugin["commands"].as_array().into_iter().flatten() {
                let Some(cid) = command["id"].as_str() else { continue };
                let label = command_label(plugin, command);
                nodes.push(Item::new(CommandRef::with_params(RUN, json!({ "id": id, "command": cid })), label).into());
            }
        }
        if !nodes.is_empty() {
            nodes.push(Node::Separator);
        }
        nodes.push(Item::new(MANAGE, tl!("Manage plug-ins…")).into());
        Some(nodes)
    }

    /// A Plug-ins menu item was chosen.
    pub(crate) fn run_plugin_menu(&mut self, command: &CommandRef) {
        if command.id == MANAGE {
            self.plugins.open = true;
            self.refresh_plugins();
            return;
        }
        let id = command.params.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let cid = command.params.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
        let Some(plugin) = self.plugins.plugin(&id).cloned() else { return };
        let Some(cmd) = plugin["commands"].as_array().into_iter().flatten().find(|c| c["id"].as_str() == Some(cid.as_str())).cloned() else {
            return;
        };
        let params: Vec<Value> = cmd["params"].as_array().cloned().unwrap_or_default();
        if params.is_empty() {
            self.run_plugin(&id, &cid, json!({}));
            return;
        }
        let values = params.iter().filter_map(|p| Some((p["name"].as_str()?.to_string(), default_value(p)))).collect();
        self.plugins.form = Some(Form { id, command: cid, title: command_label(&plugin, &cmd), params, values });
        self.plugins.open = true;
    }

    /// Run plug-in `id`'s `command` with `params` on the active document, as one undo step.
    pub(crate) fn run_plugin(&mut self, id: &str, command: &str, params: Value) {
        let Some((i, doc)) = self.active_ids() else {
            self.notify_tr("Open a document to run a plug-in on.");
            return;
        };
        // A paragraph or text box open on the page is applied first, as before Undo.
        if !self.close_text_editors(i, doc) {
            return;
        }
        let selection: Vec<usize> = self.views.get(i).map(|v| v.target_pages().into_iter().map(|p| p + 1).collect()).unwrap_or_default();
        let args = json!({ "id": id, "command": command, "params": params, "doc": doc.0, "selection": selection });
        let ran = self.plugin_tool("plugin_run", &args);
        // Whatever happened, the document may differ from what the view shows (a failed run is put back).
        if let Some(d) = self.session.get(doc)
            && let Some(view) = self.views.get_mut(i)
        {
            view.document_changed(&d.info);
            view.picked_image = None;
            view.image_selection = None;
            view.comments.selected = None;
            view.comments.history_stepped();
        }
        match ran {
            Ok(out) => {
                let notes: Vec<&str> = out["notes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let output = match &out["output"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                if !notes.is_empty() {
                    self.notify(notes.join("\n"));
                } else if !output.is_empty() {
                    self.notify(output.chars().take(500).collect::<String>());
                } else if out["panel"].is_string() {
                    self.notify_tr("PdfCraft doesn't show plug-in panels yet.");
                }
            }
            Err(e) => self.notify(e),
        }
    }

    /// Draw the Plug-ins window, when it is open.
    pub(crate) fn show_plugins(&mut self, ctx: &egui::Context) {
        self.plugins.poll();
        if !self.plugins.open {
            return;
        }
        let mut open = true;
        let mut act: Option<Action> = None;
        egui::Window::new(tl!("Plug-ins")).open(&mut open).resizable(true).default_width(520.0).show(ctx, |ui| {
            act = plugins_body(ui, &mut self.plugins);
        });
        if !open {
            self.plugins.open = false;
            self.plugins.form = None;
            self.plugins.review = None;
        }
        if let Some(act) = act {
            self.plugins_act(act, ctx);
        }
    }

    fn plugins_act(&mut self, act: Action, ctx: &egui::Context) {
        match act {
            Action::Run => {
                if let Some(form) = self.plugins.form.take() {
                    self.run_plugin(&form.id, &form.command, Value::Object(form.values));
                }
            }
            Action::CancelForm => self.plugins.form = None,
            Action::FromFile => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Install a plug-in")).add_filter(tl!("Plug-in"), &["craftplugin"]);
                    let ctx = ctx.clone();
                    self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
                        let path = path.to_string_lossy().into_owned();
                        app.plugins.start(Work::Review, "plugin_install", json!({ "path": path, "review": true }), &ctx);
                    });
                }
            }
            Action::FromRepo => {
                let repo = self.plugins.repo.trim().to_string();
                self.plugins.start(Work::Review, "plugin_install", json!({ "repo": repo, "review": true }), ctx);
            }
            Action::ListCatalog => {
                let url = self.plugins.catalog.trim().to_string();
                if url.is_empty() {
                    self.plugins.start(Work::Catalog, "plugin_catalog_list", json!({}), ctx);
                } else {
                    match self.plugin_tool("plugin_catalog_add", &json!({ "url": url })) {
                        Ok(_) => {
                            self.refresh_plugins();
                            self.plugins.start(Work::Catalog, "plugin_catalog_list", json!({ "url": url }), ctx);
                        }
                        Err(e) => self.plugins.status = Some((true, e)),
                    }
                }
            }
            Action::FromCatalog(catalog, id) => {
                self.plugins.start(Work::Review, "plugin_install", json!({ "catalog": catalog, "id": id, "review": true }), ctx);
            }
            Action::Update(id) => self.plugins.start(Work::Update, "plugin_update", json!({ "id": id }), ctx),
            Action::Remove(id) => {
                let done = self.plugin_tool("plugin_remove", &json!({ "id": id }));
                self.refresh_plugins();
                if let Err(e) = done {
                    self.plugins.status = Some((true, e));
                }
            }
            Action::Install => {
                let Some(review) = self.plugins.review.take() else { return };
                let mut args = review.request;
                if let Some(m) = args.as_object_mut() {
                    m.insert("sha256".into(), review.info["sha256"].clone());
                }
                let done = self.plugin_tool("plugin_install", &args);
                self.refresh_plugins();
                self.plugins.status = Some(match done {
                    Ok(v) => (false, crate::i18n::fmt(tl!("Installed {name}."), &[("name", v["installed"]["name"].as_str().unwrap_or_default())])),
                    Err(e) => (true, e),
                });
            }
            Action::CancelReview => self.plugins.review = None,
        }
    }
}

/// What the person asked for in the window.
enum Action {
    Run,
    CancelForm,
    FromFile,
    FromRepo,
    ListCatalog,
    FromCatalog(String, String),
    Update(String),
    Remove(String),
    Install,
    CancelReview,
}

fn plugins_body(ui: &mut egui::Ui, p: &mut Plugins) -> Option<Action> {
    if let Some((error, text)) = &p.status {
        if *error {
            ui.colored_label(ui.visuals().error_fg_color, text);
        } else {
            ui.label(text);
        }
        ui.separator();
    }
    if let Some(form) = p.form.as_mut() {
        return form_body(ui, form);
    }
    if let Some(review) = &p.review {
        return review_body(ui, review);
    }
    let busy = p.job.is_some();
    let mut act = None;
    ui.heading(tl!("Installed"));
    if p.installed().is_empty() {
        ui.label(tl!("No plug-ins are installed."));
    }
    for plugin in p.installed() {
        let id = plugin["id"].as_str().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.strong(plugin["name"].as_str().unwrap_or(id));
            ui.label(plugin["version"].as_str().unwrap_or_default());
            if ui.add_enabled(!busy, egui::Button::new(tl!("Check for update"))).clicked() {
                act = Some(Action::Update(id.to_string()));
            }
            if ui.button(tl!("Remove")).clicked() {
                act = Some(Action::Remove(id.to_string()));
            }
        });
    }
    for problem in p.listing["problems"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.colored_label(ui.visuals().warn_fg_color, problem);
    }
    ui.separator();
    ui.heading(tl!("Install"));
    ui.add_enabled_ui(!busy, |ui| {
        if ui.button(tl!("From a file…")).clicked() {
            act = Some(Action::FromFile);
        }
        ui.horizontal(|ui| {
            ui.label(tl!("GitHub repository"));
            ui.add(egui::TextEdit::singleline(&mut p.repo).hint_text("owner/repo@tag").desired_width(220.0));
            if ui.add_enabled(!p.repo.trim().is_empty(), egui::Button::new(tl!("Get"))).clicked() {
                act = Some(Action::FromRepo);
            }
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Catalog"));
            ui.add(egui::TextEdit::singleline(&mut p.catalog).hint_text("https://…/index.json").desired_width(220.0));
            if ui.button(tl!("List")).clicked() {
                act = Some(Action::ListCatalog);
            }
        });
    });
    if !p.listing["network"].as_bool().unwrap_or(false) {
        ui.label(tl!("Downloads aren't available in this build: install plug-ins from files."));
    }
    if busy {
        ui.spinner();
    }
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for pkg in &p.packages {
            ui.horizontal(|ui| {
                ui.strong(pkg["name"].as_str().unwrap_or_default());
                ui.label(pkg["version"].as_str().unwrap_or_default());
                if let Some(installed) = pkg["installed"].as_str() {
                    ui.label(crate::i18n::fmt(tl!("(installed: {version})"), &[("version", installed)]));
                }
                if ui.add_enabled(!busy, egui::Button::new(tl!("Install…"))).clicked() {
                    let (catalog, id) = (pkg["catalog"].as_str().unwrap_or_default(), pkg["id"].as_str().unwrap_or_default());
                    act = Some(Action::FromCatalog(catalog.to_string(), id.to_string()));
                }
            });
            if let Some(d) = pkg["description"].as_str().filter(|d| !d.is_empty()) {
                ui.label(d);
            }
        }
    });
    act
}

/// The install confirmation: what the plug-in is, what it may do, where it comes from and its
/// SHA-256, and plainly when no catalog in use lists it.
fn review_body(ui: &mut egui::Ui, review: &Review) -> Option<Action> {
    let info = &review.info;
    let text = |k: &str| info[k].as_str().unwrap_or_default().to_string();
    ui.heading(crate::i18n::fmt(tl!("Install {name}?"), &[("name", &text("name"))]));
    egui::Grid::new("plugin-review").num_columns(2).show(ui, |ui| {
        let rows = [
            (tl!("Version"), text("version")),
            (tl!("Author"), text("author")),
            (tl!("Licence"), text("license")),
            (tl!("Kind"), text("kind")),
            (tl!("Source"), source_text(&info["source"])),
            (tl!("SHA-256"), text("sha256")),
        ];
        for (label, value) in rows {
            ui.label(label);
            ui.label(value);
            ui.end_row();
        }
        ui.label(tl!("Access"));
        ui.label(if info["permissions"]["access"].as_str() == Some("write") {
            tl!("Reads and changes the document")
        } else {
            tl!("Reads the document")
        });
        ui.end_row();
        ui.label(tl!("Tools"));
        let commands: Vec<&str> = info["permissions"]["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        ui.label(if commands.is_empty() { tl!("Any tool PdfCraft allows plug-ins").to_string() } else { commands.join(", ") });
        ui.end_row();
    });
    if let Some(d) = info["description"].as_str().filter(|d| !d.is_empty()) {
        ui.label(d);
    }
    let warn = ui.visuals().warn_fg_color;
    match info["inCatalog"].as_bool() {
        Some(false) => {
            ui.colored_label(warn, tl!("This plug-in isn't listed in any catalog you use. Install it only if you trust where it comes from."));
        }
        None => {
            ui.colored_label(
                warn,
                tl!("This plug-in comes from a file, so no catalog vouches for it. Install it only if you trust where it comes from."),
            );
        }
        Some(true) => {}
    }
    if let Some(old) = info["replaces"].as_str() {
        ui.label(crate::i18n::fmt(tl!("It replaces version {version}."), &[("version", old)]));
    }
    if info["permissionsChanged"].as_bool() == Some(true) {
        ui.colored_label(warn, tl!("This version asks for different permissions than the one installed."));
    }
    let mut act = None;
    ui.horizontal(|ui| {
        if ui.button(tl!("Install")).clicked() {
            act = Some(Action::Install);
        }
        if ui.button(tl!("Cancel")).clicked() {
            act = Some(Action::CancelReview);
        }
    });
    act
}

/// Where a bundle comes from, as the review shows it.
fn source_text(source: &Value) -> String {
    let s = |k: &str| source[k].as_str().unwrap_or_default();
    match s("type") {
        "github" => format!("github.com/{} {}", s("repo"), s("tag")),
        "catalog" => format!("{} ({})", s("package"), s("catalog")),
        "file" => s("path").to_string(),
        _ => source.to_string(),
    }
}

/// The parameter form generated from the command's manifest `params`.
fn form_body(ui: &mut egui::Ui, form: &mut Form) -> Option<Action> {
    ui.heading(&form.title);
    egui::Grid::new("plugin-params").num_columns(2).show(ui, |ui| {
        for p in &form.params {
            let Some(name) = p["name"].as_str() else { continue };
            let label = p["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(name);
            let Some(value) = form.values.get_mut(name) else { continue };
            ui.label(label);
            param_widget(ui, name, p, value);
            ui.end_row();
        }
    });
    let mut act = None;
    ui.horizontal(|ui| {
        if ui.button(tl!("Run")).clicked() {
            act = Some(Action::Run);
        }
        if ui.button(tl!("Cancel")).clicked() {
            act = Some(Action::CancelForm);
        }
    });
    act
}

/// One parameter's control, by its kind.
fn param_widget(ui: &mut egui::Ui, name: &str, p: &Value, value: &mut Value) {
    match p["kind"].as_str().unwrap_or("text") {
        "number" | "slider" => {
            let mut n = value.as_f64().unwrap_or(0.0);
            let (min, max) = (p["min"].as_f64().unwrap_or(f64::MIN), p["max"].as_f64().unwrap_or(f64::MAX));
            if p["kind"] == "slider" && min.is_finite() && max.is_finite() && min < max {
                ui.add(egui::Slider::new(&mut n, min..=max));
            } else {
                ui.add(egui::DragValue::new(&mut n).range(min..=max));
            }
            *value = json!(n);
        }
        "boolean" => {
            let mut b = value.as_bool().unwrap_or(false);
            ui.add(widgets::checkbox(&mut b, ""));
            *value = json!(b);
        }
        "select" | "list" => {
            let current = value.as_str().unwrap_or_default().to_string();
            craft_ui::ComboBox::from_id_salt(("plugin-param", name)).selected_text(current.as_str()).show_ui(ui, |ui| {
                for option in p["options"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if ui.selectable_label(current == option, option).clicked() {
                        *value = json!(option);
                    }
                }
            });
        }
        _ => {
            let mut s = value.as_str().unwrap_or_default().to_string();
            ui.add(egui::TextEdit::singleline(&mut s).desired_width(260.0));
            *value = json!(s);
        }
    }
}
