//! The Plugins panel: install, enable, disable, remove and run
//! WebAssembly plugins (see the `pdfcraft-plugin` crate).
//!
//! This panel is deliberately generic: it knows nothing about what any
//! plugin does. Plugins describe their own commands, the panel shows a
//! button per command, and whatever a plugin asks for comes back as an
//! [`Effect`] the app applies through its ordinary paths (an edit, a new
//! document, a message). A plugin built for an unsupported API version is
//! listed with the reason and never loaded.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use pdfcraft_plugin::{Command, InstalledPlugin, LoadedPlugin};

use crate::theme::{self, Tokens};

/// Something a plugin (or the panel itself) asks the app to do.
pub enum Effect {
    /// Show a message to the user.
    Message(String),
    /// Apply `Edit::SetInfo` to the active document.
    SetDocumentInfo { key: String, value: String },
    /// Open PDF bytes as a new document.
    OpenDocument { name: String, bytes: Vec<u8> },
    /// Ask for a `.wasm` plug-in file, then install it.
    InstallRequested,
}

/// One installed plugin as the administration dialog displays it.
pub(crate) struct AdminEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub api_version: u32,
    pub enabled: bool,
    pub incompatible: Option<String>,
}

struct Entry {
    installed: InstalledPlugin,
    load_error: Option<String>,
}

/// The installed plugins and their runtime state.
pub struct PluginManager {
    dir: Option<PathBuf>,
    entries: Vec<Entry>,
    disabled: BTreeSet<String>,
    loaded: HashMap<String, LoadedPlugin>,
    problems: Vec<String>,
    /// The last message produced by a plugin run, shown in the panel.
    pub last_message: Option<String>,
}

/// No plugins folder: the app reads none until the desktop app calls
/// [`crate::PdfCraftApp::enable_plugins`], so tests and embedders never see the user's plugins.
impl Default for PluginManager {
    fn default() -> Self {
        Self::with_dir(None)
    }
}

impl PluginManager {
    /// The plugins installed in `dir`, read now.
    pub fn with_dir(dir: Option<PathBuf>) -> Self {
        let mut manager =
            Self { dir, entries: Vec::new(), disabled: BTreeSet::new(), loaded: HashMap::new(), problems: Vec::new(), last_message: None };
        manager.refresh();
        manager
    }

    /// Re-read the plugins folder (after an install, a removal, or on demand).
    pub fn refresh(&mut self) {
        self.loaded.clear();
        let Some(dir) = self.dir.clone() else {
            self.entries = Vec::new();
            self.problems = vec!["No plugins folder is available on this system.".into()];
            return;
        };
        let (found, problems) = pdfcraft_plugin::discover(&dir);
        self.problems = problems;
        self.disabled = pdfcraft_plugin::load_disabled(&dir);
        self.entries = found
            .into_iter()
            .map(|installed| {
                let load_error = installed.manifest.compatibility();
                Entry { installed, load_error }
            })
            .collect();
    }

    /// How many plugins are installed (the panel title's count).
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn dir(&self) -> Option<&PathBuf> {
        self.dir.as_ref()
    }

    pub(crate) fn entries_is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Flat, cloneable view of the installed plugins for the admin dialog.
    pub(crate) fn entries_snapshot(&self) -> Vec<AdminEntry> {
        self.entries
            .iter()
            .map(|entry| {
                let m = &entry.installed.manifest;
                AdminEntry {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    description: m.description.clone(),
                    author: m.author.clone(),
                    api_version: pdfcraft_plugin::API_VERSION,
                    enabled: !self.disabled.contains(&m.id),
                    incompatible: entry.load_error.clone(),
                }
            })
            .collect()
    }

    /// Enable or disable a plugin, and persist the choice in `disabled.json`.
    fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        if enabled {
            self.disabled.remove(id);
        } else {
            self.disabled.insert(id.to_string());
            self.loaded.remove(id);
        }
        match self.dir.as_deref() {
            Some(dir) => pdfcraft_plugin::save_disabled(dir, &self.disabled).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.installed.manifest.id == id)
    }

    /// Whether the plugin `id` is installed, enabled and compatible.
    pub(crate) fn runnable(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|e| e.load_error.is_none() && !self.disabled.contains(id))
    }

    fn ensure_loaded(&mut self, id: &str) -> Result<&mut LoadedPlugin, String> {
        let Some(entry) = self.entries.iter().find(|e| e.installed.manifest.id == id) else {
            return Err(format!("the plugin {id} is no longer installed"));
        };
        if let Some(reason) = &entry.load_error {
            return Err(reason.clone());
        }
        if !self.loaded.contains_key(id) {
            let loaded = entry.installed.load().map_err(|e| e.to_string())?;
            self.loaded.insert(id.to_string(), loaded);
        }
        self.loaded.get_mut(id).ok_or_else(|| format!("the plugin {id} could not be loaded"))
    }
}

/// The `invoke` context for the active document, shared by the panel and
/// the Plugins menu.
pub(crate) fn context_for(doc: &pdfcraft_engine::Document, current: usize) -> serde_json::Value {
    serde_json::json!({
        "document": {
            "name": doc.name,
            "page_count": doc.info.pages.len(),
            "current_page": current,
            "page": doc.info.pages.get(current).map(|p| serde_json::json!({"width": p.width, "height": p.height})),
        }
    })
}

/// Installed plugins as the Plugins menu shows them: id, name, and the
/// commands of those that may run (empty for a disabled or incompatible one).
pub(crate) struct MenuEntry<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub commands: &'a [Command],
}

impl PluginManager {
    /// The menu entries, in folder order. Commands come from the manifest embedded in the
    /// module, so listing them loads nothing.
    pub(crate) fn menu_entries(&self) -> Vec<MenuEntry<'_>> {
        self.entries
            .iter()
            .map(|entry| {
                let m = &entry.installed.manifest;
                let commands: &[Command] = if self.runnable(&m.id) { &m.commands } else { &[] };
                MenuEntry { id: &m.id, name: &m.name, commands }
            })
            .collect()
    }

    /// Run one command and translate the guest's answer into effects.
    pub(crate) fn invoke(&mut self, id: &str, command: &str, context: &serde_json::Value) -> Vec<Effect> {
        let mut effects = Vec::new();
        match self.ensure_loaded(id) {
            Ok(loaded) => match loaded.invoke(command, context) {
                Ok(response) => {
                    if let Some(message) = &response.message {
                        self.last_message = Some(message.clone());
                    }
                    for action in response.actions {
                        match action {
                            pdfcraft_plugin::Action::ShowMessage { text } => {
                                self.last_message = Some(text.clone());
                                effects.push(Effect::Message(text));
                            }
                            pdfcraft_plugin::Action::SetDocumentInfo { key, value } => {
                                effects.push(Effect::SetDocumentInfo { key, value });
                            }
                            pdfcraft_plugin::Action::OpenDocument { name, data_base64 } => {
                                use base64::Engine as _;
                                match base64::engine::general_purpose::STANDARD.decode(&data_base64) {
                                    Ok(bytes) => effects.push(Effect::OpenDocument { name, bytes }),
                                    Err(e) => effects.push(Effect::Message(format!("the plugin sent an unreadable document: {e}"))),
                                }
                            }
                        }
                    }
                }
                Err(e) => effects.push(Effect::Message(e.to_string())),
            },
            Err(e) => effects.push(Effect::Message(e)),
        }
        effects
    }
}

/// The body of the Plugins panel. `context` describes the active document
/// for the guest; effects are applied by the caller.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, manager: &mut PluginManager, context: &serde_json::Value) -> Vec<Effect> {
    let mut effects = Vec::new();
    ui.horizontal(|ui| {
        if ui.button(tl!("Install a plugin…")).clicked() {
            effects.push(Effect::InstallRequested);
        }
        if ui.button(tl!("Refresh")).clicked() {
            manager.refresh();
        }
    });
    if let Some(dir) = manager.dir() {
        ui.label(egui::RichText::new(dir.display().to_string()).color(t.text_faint).font(theme::regular(11.5)));
    }
    ui.add_space(6.0);

    for problem in &manager.problems {
        ui.label(egui::RichText::new(problem.clone()).color(t.text_muted).font(theme::regular(12.0)));
    }
    if manager.entries.is_empty() {
        ui.label(
            egui::RichText::new(tl!("No plugins installed. A plugin is a single .wasm file; install one above."))
                .color(t.text_muted)
                .font(theme::regular(12.5)),
        );
    }

    let mut toggle: Option<(String, bool)> = None;
    let mut remove: Option<String> = None;
    let mut run: Option<(String, String)> = None;
    for entry in &manager.entries {
        let manifest = &entry.installed.manifest;
        let enabled = !manager.disabled.contains(&manifest.id);
        let incompatible = entry.load_error.clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&manifest.name).font(theme::semibold(13.5)));
            ui.label(egui::RichText::new(format!("v{}", manifest.version)).color(t.text_faint).font(theme::regular(12.0)));
        });
        if !manifest.description.is_empty() {
            ui.label(egui::RichText::new(&manifest.description).color(t.text_muted).font(theme::regular(12.5)));
        }
        ui.horizontal(|ui| {
            let mut on = enabled;
            if ui.add(crate::widgets::checkbox(&mut on, tl!("Enabled"))).changed() {
                toggle = Some((manifest.id.clone(), on));
            }
            if ui.button(tl!("Remove")).clicked() {
                remove = Some(manifest.id.clone());
            }
        });
        if let Some(reason) = &incompatible {
            ui.label(egui::RichText::new(reason.clone()).color(t.text_muted).font(theme::regular(12.0)));
        } else if enabled {
            for command in &manifest.commands {
                let tip = if command.description.is_empty() { &command.label } else { &command.description };
                if ui.button(&command.label).on_hover_text(tip).clicked() {
                    run = Some((manifest.id.clone(), command.id.clone()));
                }
            }
        }
    }

    if let Some((id, enabled)) = toggle
        && let Err(e) = manager.set_enabled(&id, enabled)
    {
        effects.push(Effect::Message(e));
    }
    if let Some(id) = remove
        && let Some(dir) = manager.dir.clone()
    {
        match pdfcraft_plugin::uninstall(&dir, &id) {
            Ok(()) => effects.push(Effect::Message(format!("Removed {id}."))),
            Err(e) => effects.push(Effect::Message(e.to_string())),
        }
        manager.refresh();
    }
    if let Some((id, command)) = run {
        effects.extend(manager.invoke(&id, &command, context));
    }

    if let Some(message) = &manager.last_message {
        ui.add_space(10.0);
        ui.separator();
        ui.label(egui::RichText::new(message.clone()).font(theme::regular(12.5)));
    }
    effects
}

impl crate::PdfCraftApp {
    /// Read the plugins from this installation's plugins folder: the portable data folder in
    /// portable mode, otherwise the per-user data folder ([`pdfcraft_plugin::default_plugins_dir`]).
    pub fn enable_plugins(&mut self) {
        self.plugins = PluginManager::with_dir(pdfcraft_plugin::default_plugins_dir(crate::portable::data_dir()));
    }

    /// Apply effects returned by a plugin run (or the panel) through the
    /// app's ordinary paths.
    pub(crate) fn apply_plugin_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Message(message) => self.notify(message),
                Effect::SetDocumentInfo { key, value } => {
                    self.apply_edit(pdfcraft_engine::Edit::SetInfo { key, value });
                }
                Effect::OpenDocument { name, bytes } => {
                    if let Err(e) = self.open_bytes(&name, None, bytes) {
                        self.notify_error(e);
                    }
                }
                Effect::InstallRequested => self.install_plugin_dialog(),
            }
        }
    }

    /// The install flow shared by the Plugins panel and the Plugins menu.
    pub(crate) fn install_plugin_dialog(&mut self) {
        // The web build has no plugins folder to install into.
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("No plugins folder is available on this system.");
        #[cfg(not(target_arch = "wasm32"))]
        let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Install a plugin").to_string()).add_filter("WebAssembly plug-in", &["wasm"]);
        #[cfg(not(target_arch = "wasm32"))]
        self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
            let Some(dir) = app.plugins.dir().cloned() else {
                app.notify_tr("No plugins folder is available on this system.");
                return;
            };
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| pdfcraft_plugin::install_module(&bytes, &dir).map_err(|e| e.to_string()))
            {
                Ok(manifest) => {
                    app.plugins.refresh();
                    app.notify_fmt("Installed {name} {version}", &[("name", &manifest.name), ("version", &manifest.version)]);
                }
                Err(e) => app.notify_error(e),
            }
        });
    }

    /// Run one plugin command from the Plugins menu against the active document.
    pub(crate) fn run_plugin_command(&mut self, plugin: &str, command: &str) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let current = self.active.and_then(|i| self.views.get(i)).map(|v| v.current).unwrap_or(0);
        let context = context_for(doc, current);
        let effects = self.plugins.invoke(plugin, command, &context);
        self.apply_plugin_effects(effects);
    }
}

/// The Plugins administration dialog (Plugins ▸ Manage plugins…).
///
/// Unlike the right-hand panel, this dialog exists with no document open:
/// installing, enabling and removing plugins is application administration,
/// not document work. Running commands stays in the Plugins menu and the
/// side panel, where a document context exists.
pub(crate) fn admin_body(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut install = false;
    let mut toggle: Option<(String, bool)> = None;
    let mut remove: Option<String> = None;

    {
        let manager = &mut app.plugins;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Plugins")).font(theme::semibold(18.0)));
            ui.label(egui::RichText::new(format!("{}", manager.count())).color(t.text_faint).font(theme::regular(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tl!("Refresh")).clicked() {
                    manager.refresh();
                }
                if ui.button(tl!("Install a plugin…")).clicked() {
                    install = true;
                }
            });
        });
        if let Some(dir) = manager.dir() {
            ui.label(egui::RichText::new(dir.display().to_string()).color(t.text_faint).font(theme::regular(11.5)));
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(tl!(
                "Plugins are sandboxed WebAssembly packages. They survive PdfCraft updates as long as their API version is supported."
            ))
            .color(t.text_muted)
            .font(theme::regular(12.5)),
        );
        ui.separator();

        for problem in &manager.problems {
            ui.label(egui::RichText::new(problem.clone()).color(t.text_muted).font(theme::regular(12.0)));
        }
        if manager.entries_is_empty() {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(tl!("No plugins installed yet. Install a .wasm plug-in to extend PdfCraft."))
                    .color(t.text_muted)
                    .font(theme::regular(13.0)),
            );
        } else {
            egui::ScrollArea::vertical().max_height(440.0).auto_shrink([false, true]).show(ui, |ui| {
                for entry in manager.entries_snapshot() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&entry.name).font(theme::semibold(14.0)));
                            ui.label(egui::RichText::new(format!("v{}", entry.version)).color(t.text_faint).font(theme::regular(12.0)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let (label, color) = if entry.incompatible.is_some() {
                                    (tl!("Incompatible").to_string(), egui::Color32::from_rgb(0xC8, 0x3A, 0x3A))
                                } else if entry.enabled {
                                    (tl!("Enabled").to_string(), egui::Color32::from_rgb(0x1E, 0x8A, 0x5A))
                                } else {
                                    (tl!("Disabled").to_string(), t.text_faint)
                                };
                                ui.label(egui::RichText::new(label).color(color).font(theme::semibold(12.0)));
                            });
                        });
                        if !entry.description.is_empty() {
                            ui.label(egui::RichText::new(&entry.description).color(t.text_muted).font(theme::regular(12.5)));
                        }
                        let mut meta = format!("{} · API {}", entry.id, entry.api_version);
                        if !entry.author.is_empty() {
                            meta.push_str(&format!(" · {}", entry.author));
                        }
                        ui.label(egui::RichText::new(meta).color(t.text_faint).font(egui::FontId::monospace(11.0)));
                        if let Some(reason) = &entry.incompatible {
                            ui.label(egui::RichText::new(reason.clone()).color(egui::Color32::from_rgb(0xC8, 0x3A, 0x3A)).font(theme::regular(12.0)));
                        }
                        ui.horizontal(|ui| {
                            let mut on = entry.enabled;
                            if ui.add_enabled(entry.incompatible.is_none(), crate::widgets::checkbox(&mut on, tl!("Enabled"))).changed() {
                                toggle = Some((entry.id.clone(), on));
                            }
                            if ui.button(tl!("Remove")).clicked() {
                                remove = Some(entry.id.clone());
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        }
    }

    if install {
        app.install_plugin_dialog();
    }
    if let Some((id, enabled)) = toggle
        && let Err(e) = app.plugins.set_enabled(&id, enabled)
    {
        app.notify_error(e);
    }
    if let Some(id) = remove
        && let Some(dir) = app.plugins.dir().cloned()
    {
        match pdfcraft_plugin::uninstall(&dir, &id) {
            Ok(()) => app.notify(format!("Removed {id}.")),
            Err(e) => app.notify_error(e.to_string()),
        }
        app.plugins.refresh();
    }
}
