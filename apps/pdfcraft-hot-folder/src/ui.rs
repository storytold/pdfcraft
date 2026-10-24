use crate::i18n::{LANGUAGES, Language};
use chrono::{Local, TimeZone};
use pdfcraft_engine::{hot_folder as hf, hot_folder_output as output, ocr};
use pdfcraft_hot_folder_service as service;
use std::path::PathBuf;

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("Inter".into(), std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "Inter".into());
    for face in pdfcraft_fonts::ui_cjk_fonts(false) {
        let name = face.name();
        fonts.font_data.insert(name.clone(), std::sync::Arc::new(egui::FontData::from_static(face.bytes)));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push(name);
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::light());
}

pub struct HotFolderApp {
    path: PathBuf,
    config: hf::Config,
    state: hf::State,
    selected: usize,
    start_text: String,
    language: Language,
    error: Option<String>,
    /// The background worker's last diagnostic, kept apart from `error` so a refresh never
    /// hides why the user's own action failed, and cleared when the worker recovers.
    worker_error: Option<String>,
    saved: bool,
    last_refresh: std::time::Instant,
    errors_only: bool,
    startup_enabled: bool,
    service_state: Option<service::Status>,
    service_account: String,
    load_error: Option<String>,
    saved_snapshot: Vec<u8>,
    disk_snapshot: Option<Vec<u8>>,
    close_prompt: bool,
    allow_close: bool,
    source_ui: crate::source_ui::SourceUi,
    /// Tests never launch an external process.
    #[cfg(test)]
    inline: bool,
}

impl HotFolderApp {
    pub fn new(path: PathBuf) -> Self {
        let loaded = if path.exists() { hf::load_config(&path) } else { Ok(hf::Config::default()) };
        let error = loaded.as_ref().err().cloned();
        let saved_snapshot = loaded.as_ref().ok().and_then(|c| serde_json::to_vec(c).ok()).unwrap_or_default();
        let disk_snapshot = read_disk(&path).ok().flatten();
        let startup_enabled = std::env::current_exe().is_ok_and(|exe| crate::startup::enabled(&exe, &path));
        let service_state = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| service::status(&exe, &path));
        let initial_error = error.clone().or_else(|| service_state.as_ref().err().cloned());
        let mut app = Self {
            state: hf::read_state(&path).unwrap_or_default(),
            path: path.clone(),
            config: loaded.unwrap_or_default(),
            selected: usize::MAX,
            start_text: String::new(),
            language: Language::system(sys_locale::get_locale().as_deref()),
            load_error: error.clone(),
            error: initial_error,
            worker_error: None,
            saved: false,
            saved_snapshot,
            disk_snapshot,
            close_prompt: false,
            allow_close: false,
            source_ui: Default::default(),
            last_refresh: std::time::Instant::now(),
            errors_only: false,
            startup_enabled,
            service_state: service_state.ok(),
            service_account: String::new(),
            #[cfg(test)]
            inline: false,
        };
        app.select(0);
        app
    }

    fn select(&mut self, index: usize) {
        if let Err(error) = self.update_start() {
            self.error = Some(error);
            return;
        }
        self.selected = index;
        self.source_ui = Default::default();
        self.start_text = self
            .config
            .tasks
            .get(index)
            .and_then(|t| (t.start_at > 0).then_some(t.start_at))
            .and_then(|v| i64::try_from(v).ok())
            .and_then(|v| Local.timestamp_opt(v, 0).single())
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
    }

    fn new_task(&mut self, copy: bool) {
        let mut task = if copy { self.config.tasks.get(self.selected).cloned().unwrap_or_default() } else { hf::Task::default() };
        let micros = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros());
        task.id = format!("task-{micros}-{}", self.config.tasks.len());
        task.enabled = false;
        if !copy {
            task.name = self.language.t("New task").into();
            task.settings.language = ocr::language_for_locale(sys_locale::get_locale().as_deref()).into();
        }
        self.config.tasks.push(task);
        self.select(self.config.tasks.len().saturating_sub(1));
        self.saved = false;
    }

    fn update_start(&mut self) -> Result<(), String> {
        if let Some(task) = self.config.tasks.get_mut(self.selected) {
            task.start_at = if self.start_text.trim().is_empty() {
                0
            } else {
                let naive = chrono::NaiveDateTime::parse_from_str(self.start_text.trim(), "%Y-%m-%d %H:%M")
                    .map_err(|_| self.language.t("Invalid or ambiguous local start time"))?;
                let value = Local.from_local_datetime(&naive).single().ok_or(self.language.t("Invalid or ambiguous local start time"))?.timestamp();
                u64::try_from(value).map_err(|_| self.language.t("Invalid or ambiguous local start time"))?
            };
        }
        Ok(())
    }

    fn dirty(&self) -> bool {
        let saved = serde_json::to_vec(&self.config).ok();
        let date = self
            .config
            .tasks
            .get(self.selected)
            .and_then(|t| i64::try_from(t.start_at).ok())
            .filter(|t| *t > 0)
            .and_then(|t| Local.timestamp_opt(t, 0).single())
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        saved.as_ref() != Some(&self.saved_snapshot) || date != self.start_text
    }

    fn save(&mut self) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        self.update_start()?;
        if read_disk(&self.path)? != self.disk_snapshot {
            return Err(self.language.t("Settings changed in another window. Reload before saving.").into());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        hf::save_config(&self.path, &self.config, None)?;
        self.saved = true;
        self.saved_snapshot = serde_json::to_vec(&self.config).map_err(|e| e.to_string())?;
        self.disk_snapshot = read_disk(&self.path)?;
        Ok(())
    }

    fn start(&mut self, immediate: bool) -> Result<(), String> {
        if immediate {
            self.start_text.clear();
        }
        self.save()?;
        let id = self.config.tasks.get(self.selected).ok_or("no task selected")?.id.clone();
        if immediate {
            hf::run_now(&self.path, &id, None)?;
        } else {
            hf::reset(&self.path, &id, None)?;
        }
        if let Some(task) = self.config.tasks.get_mut(self.selected) {
            task.enabled = true;
        }
        self.save()?;
        #[cfg(test)]
        if self.inline {
            return Ok(());
        }
        if let Err(error) = crate::launch_worker(&self.path) {
            if let Some(task) = self.config.tasks.get_mut(self.selected) {
                task.enabled = false;
            }
            return match self.save() {
                Ok(()) => Err(error),
                Err(save) => Err(format!("{error}; {save}")),
            };
        }
        Ok(())
    }

    fn stop(&mut self, all: bool) -> Result<(), String> {
        // Stop uses persisted settings, so an unrelated incomplete draft cannot prevent it.
        let mut config = hf::load_config(&self.path)?;
        let id = self.config.tasks.get(self.selected).map(|t| t.id.clone());
        for task in &mut config.tasks {
            if all || Some(&task.id) == id.as_ref() {
                task.enabled = false;
            }
        }
        hf::save_config(&self.path, &config, None)?;
        self.disk_snapshot = read_disk(&self.path)?;
        self.saved_snapshot = serde_json::to_vec(&config).map_err(|e| e.to_string())?;
        for task in &mut self.config.tasks {
            if all || Some(&task.id) == id.as_ref() {
                task.enabled = false;
            }
        }
        Ok(())
    }

    fn refresh(&mut self) {
        self.source_ui.tick(&self.path);
        if self.last_refresh.elapsed().as_millis() < 1000 {
            return;
        }
        self.last_refresh = std::time::Instant::now();
        match hf::read_state(&self.path) {
            Ok(state) => self.state = state,
            Err(e) => self.error = Some(e),
        }
        self.worker_error = match hf::read_worker_error(&self.path) {
            Ok(error) => error,
            Err(error) => Some(error),
        };
    }

    fn import_tasks(&mut self) -> Result<(), String> {
        let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else {
            return Ok(());
        };
        let mut imported = hf::load_config(&path)?;
        // Importing a task never starts it or deletes a source implicitly.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |v| v.as_micros());
        for (index, task) in imported.tasks.iter_mut().enumerate() {
            task.enabled = false;
            if self.config.tasks.iter().any(|existing| existing.id == task.id) {
                task.id = format!("imported-{stamp}-{index}");
            }
        }
        hf::validate(&imported, None)?;
        let mut merged = self.config.clone();
        merged.tasks.extend(imported.tasks);
        hf::validate(&merged, None)?;
        self.config = merged;
        self.save()
    }

    fn service_action(&mut self, action: &str) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        match action {
            "start" => service::start(&exe, &self.path)?,
            "stop" => service::stop(&exe, &self.path)?,
            "enable" => crate::startup::set_enabled(true, &exe, &self.path)?,
            "disable" => crate::startup::set_enabled(false, &exe, &self.path)?,
            "install" => {
                self.save()?;
                service::install(&exe, &self.path, &self.service_account)?;
            }
            "uninstall" => service::uninstall(&exe, &self.path)?,
            _ => {}
        }
        let status = service::status(&exe, &self.path)?;
        self.startup_enabled = status.enabled;
        self.service_state = Some(status);
        Ok(())
    }

    pub fn show(&mut self, root: &mut egui::Ui) {
        self.refresh();
        let ctx = root.ctx();
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        let lang = self.language;
        let t = |key| lang.t(key);
        egui::Panel::top("header").show(root, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("PdfCraft Hot Folder");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("ui-language").selected_text(LANGUAGES.get(self.language.0).map_or("English", |l| l.1)).show_ui(
                        ui,
                        |ui| {
                            for (index, (_, name)) in LANGUAGES.iter().enumerate() {
                                ui.selectable_value(&mut self.language, Language(index), *name);
                            }
                        },
                    );
                    ui.label(t("Language"));
                });
            });
            ui.label(t("Processing continues when this window or PdfCraft is closed. Stop tasks here to pause it."));
            ui.add_space(8.0);
        });
        if self.error.is_some() || self.worker_error.is_some() {
            egui::Panel::top("error").show(root, |ui| {
                ui.colored_label(egui::Color32::DARK_RED, t("Error details"));
                egui::ScrollArea::vertical().id_salt("error-details").max_height(80.0).show(ui, |ui| {
                    for error in self.error.iter().chain(&self.worker_error) {
                        ui.label(error);
                    }
                });
            });
        }
        egui::Panel::left("tasks").resizable(true).default_size(250.0).show(root, |ui| {
            egui::ScrollArea::vertical().id_salt("task-list").show(ui, |ui| {
                ui.heading(t("Tasks"));
                if ui.add_enabled(self.config.tasks.len() < 32, egui::Button::new(t("New task"))).clicked() {
                    self.new_task(false);
                }
                if ui.button(t("Import tasks…")).clicked() {
                    self.error = self.import_tasks().err();
                }
                if ui.button(t("Export tasks…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("hot-folder-tasks.json").save_file()
                {
                    self.error = hf::save_config(&path, &self.config, None).err();
                }
                if ui.button(t("Discard edits and reload")).clicked() {
                    let language = self.language;
                    *self = Self::new(self.path.clone());
                    self.language = language;
                }
                ui.add(crate::widgets::checkbox(&mut self.errors_only, t("Show errors only")));
                let mut selected = self.selected;
                for (index, task) in self.config.tasks.iter().enumerate() {
                    let status = self.state.tasks.get(&task.id);
                    if self.errors_only && status.is_none_or(|s| s.error.is_none()) {
                        continue;
                    }
                    let label = if !task.enabled {
                        "Stopped"
                    } else if status.is_some_and(|s| s.error.is_some()) {
                        "Failed"
                    } else if status.is_some_and(|s| s.finished) {
                        "Completed"
                    } else if task.start_at > hf::now() {
                        "Scheduled"
                    } else {
                        "Watching"
                    };
                    ui.selectable_value(&mut selected, index, format!("{}\n{}", task.name, t(label)));
                    ui.small(task.input.display().to_string());
                    if task.enabled {
                        let next = status.map_or(task.start_at, |s| s.next_check.max(task.start_at));
                        if next > 0
                            && let Ok(next) = i64::try_from(next)
                            && let Some(date) = Local.timestamp_opt(next, 0).single()
                        {
                            ui.small(format!("{}: {}", t("Next check"), date.format("%Y-%m-%d %H:%M:%S")));
                        }
                    }
                }
                if selected != self.selected {
                    self.select(selected);
                }
                ui.separator();
                if ui.button(t("Stop all")).clicked() {
                    self.error = self.stop(true).err();
                }
                ui.collapsing(t("System service"), |ui| {
                    let installed = self.service_state.as_ref().is_some_and(|s| s.installed);
                    let running = self.service_state.as_ref().is_some_and(|s| s.running);
                    ui.label(t(if self.service_state.is_none() {
                        "Status unavailable"
                    } else if running {
                        "Running"
                    } else if installed {
                        "Stopped"
                    } else {
                        "Not installed"
                    }));
                    ui.small(self.path.display().to_string());
                    if let Some(state) = &self.service_state
                        && installed
                    {
                        ui.label(format!("{}: {}", t("Service account"), state.account));
                    }
                    let mut action = None;
                    let mut enabled = self.startup_enabled;
                    if ui.add_enabled(installed, crate::widgets::checkbox(&mut enabled, t("Start with the system"))).changed() {
                        action = Some(if enabled { "enable" } else { "disable" });
                    }
                    if ui.add_enabled(installed && !running, egui::Button::new(t("Start service"))).clicked() {
                        action = Some("start");
                    }
                    if ui.add_enabled(installed && running, egui::Button::new(t("Stop service"))).clicked() {
                        action = Some("stop");
                    }
                    if ui.button(t("Refresh status")).clicked() {
                        action = Some("status");
                    }
                    #[cfg(unix)]
                    if !installed {
                        ui.label(t("Service account"));
                        ui.text_edit_singleline(&mut self.service_account);
                        if ui.button(t("Install service")).clicked() {
                            action = Some("install");
                        }
                    }
                    #[cfg(unix)]
                    if installed && ui.button(t("Uninstall service")).clicked() {
                        action = Some("uninstall");
                    }
                    #[cfg(windows)]
                    ui.weak(t("Install or remove the system service with the optional Windows installer component."));
                    ui.weak(t("Service changes require administrator rights. Grant the service account access to the selected folders."));
                    ui.weak(t("To edit protected service settings, reopen Hot Folder as administrator with the same configuration path."));
                    if let Some(action) = action {
                        self.error = self.service_action(action).err();
                    }
                });
            });
        });
        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut save = false;
                let mut start = false;
                let mut scheduled = false;
                let mut stop = false;
                let mut duplicate = false;
                let mut delete = false;
                let mut retry = false;
                let can_add = self.config.tasks.len() < 32;
                if let Some(task) = self.config.tasks.get_mut(self.selected) {
                    ui.horizontal_wrapped(|ui| {
                        save = ui.add_enabled(!task.enabled, egui::Button::new(t("Save"))).clicked();
                        start = ui.add_enabled(!task.enabled && (ocr::available() || !needs_ocr(task)), egui::Button::new(t("Start now"))).clicked();
                        scheduled = ui
                            .add_enabled(!task.enabled && (ocr::available() || !needs_ocr(task)), egui::Button::new(t("Start on schedule")))
                            .clicked();
                        stop = ui.add_enabled(task.enabled, egui::Button::new(t("Stop"))).clicked();
                        duplicate = ui.add_enabled(can_add, egui::Button::new(t("Duplicate"))).clicked();
                        delete = ui.add_enabled(!task.enabled, egui::Button::new(t("Delete task"))).clicked();
                    });
                    if task.enabled {
                        ui.label(t("Stop this task before editing."));
                    }
                    if !ocr::available() && needs_ocr(task) {
                        ui.colored_label(egui::Color32::DARK_RED, t("OCR models are missing. Reinstall PdfCraft with OCR models."));
                    }
                    ui.add_enabled_ui(!task.enabled, |ui| {
                        ui.label(t("Task name"));
                        ui.text_edit_singleline(&mut task.name);
                        self.source_ui.show(ui, task, lang);
                        let local = matches!(task.source, pdfcraft_engine::hot_folder_sources::SourceConfig::Local);
                        folder(ui, &mut task.input, t(if local { "Input folder" } else { "Staging folder" }), lang);
                        folder(ui, &mut task.output, t("Output folder"), lang);
                        if local {
                            ui.add(crate::widgets::checkbox(&mut task.recursive, t("Include subfolders")));
                        }
                        ui.horizontal(|ui| {
                            ui.add(crate::widgets::checkbox(&mut task.pdf, "PDF"));
                            ui.add(crate::widgets::checkbox(
                                &mut task.images,
                                t(if local { "Images (PNG, JPEG, TIFF, BMP)" } else { "Images (PNG, JPEG, TIFF)" }),
                            ));
                        });
                        ui.add(crate::widgets::checkbox(&mut task.analysis_only, t("Analyze without OCR")));
                        if task.analysis_only {
                            ui.weak(t("Creates JSON layout reports. OCR, image changes and blank-page removal are disabled."));
                        }
                        output_steps(ui, &mut task.output_plan, lang, task.analysis_only);
                        ui.separator();
                        ui.heading(t("Schedule"));
                        ui.horizontal(|ui| {
                            crate::widgets::radio_value(ui, &mut task.once, true, t("Run once"));
                            crate::widgets::radio_value(ui, &mut task.once, false, t("Recurring"));
                        });
                        if !task.once {
                            calendar(ui, &mut task.schedule, lang);
                        }
                        ui.label(t("Start time (local)"));
                        ui.text_edit_singleline(&mut self.start_text);
                        ui.weak(t("Leave blank to start now; YYYY-MM-DD HH:MM"));
                        ui.horizontal(|ui| {
                            ui.label(t("Check interval (seconds)"));
                            ui.add(egui::DragValue::new(&mut task.interval_seconds).range(1..=86400));
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("Wait for stable files (seconds)"));
                            ui.add(egui::DragValue::new(&mut task.settle_seconds).range(1..=300));
                        });
                        ui.separator();
                        if local {
                            ui.heading(t("Original files"));
                            ui.horizontal_wrapped(|ui| {
                                crate::widgets::radio_value(ui, &mut task.originals, hf::Originals::Keep, t("Keep"));
                                crate::widgets::radio_value(ui, &mut task.originals, hf::Originals::Move, t("Move after success"));
                                crate::widgets::radio_value(ui, &mut task.originals, hf::Originals::Delete, t("Delete after success"));
                            });
                            if task.originals == hf::Originals::Move {
                                folder(ui, &mut task.archive, t("Archive folder"), lang);
                            }
                            ui.weak(t("Only after the result is saved. Errors always keep the original."));
                        }
                        ui.separator();
                        if task.analysis_only {
                            ui.label(t("Pages to process (empty = all; example: 1-3,5)"));
                            ui.text_edit_singleline(&mut task.preprocessing.pages);
                        } else {
                            ui.heading(t("Scan and text recognition"));
                            ui.horizontal(|ui| {
                                ui.label(t("Document language"));
                                let name = ocr::LANGUAGES.iter().find(|l| l.0 == task.settings.language).map_or("English", |l| l.1);
                                egui::ComboBox::from_id_salt("ocr-language").height(480.0).selected_text(name).show_ui(ui, |ui| {
                                    for (code, name) in ocr::LANGUAGES {
                                        ui.selectable_value(&mut task.settings.language, (*code).into(), *name);
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                ui.label(t("Resolution (dpi)"));
                                ui.add(egui::DragValue::new(&mut task.settings.dpi).range(72.0..=600.0));
                            });
                            ui.add(crate::widgets::checkbox(&mut task.settings.recognize, t("Add searchable text")));
                            ui.add(crate::widgets::checkbox(&mut task.settings.skip_text_pages, t("Skip pages that already contain text")));
                            ui.add(crate::widgets::checkbox(&mut task.settings.auto_rotate, t("Automatically correct page orientation")));
                            ui.add(crate::widgets::checkbox(&mut task.settings.deskew, t("Straighten skewed scans")));
                            ui.add(crate::widgets::checkbox(&mut task.settings.auto_crop, t("Crop to detected paper edges")));
                            ui.add_enabled_ui(task.settings.auto_crop, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(t("Safety margin (pt)"));
                                    ui.add(egui::DragValue::new(&mut task.settings.crop_margin).range(0.0..=20.0));
                                });
                            });
                            ui.weak(t("Uncertain orientations and paper edges are kept unchanged."));
                            image_preparation(ui, &mut task.preprocessing, lang);
                            ui.add(crate::widgets::checkbox(&mut task.remove_blank_pages, t("Remove completely white pages")));
                            if task.remove_blank_pages {
                                let mut archive = task.blank_archive.is_some();
                                if ui.add(crate::widgets::checkbox(&mut archive, t("Save removed blank pages separately"))).changed() {
                                    task.blank_archive = archive.then(PathBuf::new);
                                }
                                if let Some(path) = &mut task.blank_archive {
                                    folder(ui, path, t("Blank page archive"), lang);
                                }
                            }
                            ui.weak(t("Faint marks and uncertain blank pages are preserved."));
                        }
                    });
                    ui.separator();
                    if let Some(status) = self.state.tasks.get(&task.id) {
                        ui.label(format!(
                            "{}: {}    {}: {}    {}: {}",
                            t("Completed"),
                            status.completed,
                            t("Waiting"),
                            status.waiting,
                            t("Failed"),
                            status.failed
                        ));
                        if let Some(output) = &status.last_output {
                            ui.label(output.display().to_string());
                        }
                        if let Some(summary) = &status.last_optimization {
                            ui.label(format!("{}: {} → {} {}", t("PDF optimization"), summary.bytes_before, summary.bytes_after, t("bytes")));
                            ui.label(format!("{}: {} / {}", t("PDFs reduced"), summary.changed, summary.documents));
                        }
                        if let Some(error) = &status.error {
                            ui.collapsing(t("Error details"), |ui| {
                                ui.label(error);
                            });
                        }
                        retry = ui.add_enabled(!task.enabled, egui::Button::new(t("Retry failed files"))).clicked();
                    }
                    if ui.button(t("Open output folder")).clicked() {
                        self.error = open_folder(&task.output).err();
                    }
                    ui.collapsing(t("Log"), |ui| {
                        for entry in self.state.log.iter().rev().filter(|e| e.task == task.id).take(100) {
                            let time = i64::try_from(entry.at)
                                .ok()
                                .and_then(|v| Local.timestamp_opt(v, 0).single())
                                .map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or_default();
                            ui.label(format!("{time} — {}", entry.input.as_ref().map(|v| v.display().to_string()).unwrap_or_default()));
                            if let Some(error) = &entry.error {
                                ui.label(format!("{}: {error}", t("Error details")));
                            }
                            if let Some(output) = &entry.output {
                                ui.label(output.display().to_string());
                            }
                            ui.separator();
                        }
                    });
                } else {
                    ui.label(t("Create a task to process new scans automatically."));
                }
                if save {
                    self.error = self.save().err();
                }
                if self.source_ui.has_pending() {
                    if let Err(error) = self.save() {
                        self.source_ui.cancel_pending();
                        self.error = Some(error);
                    } else {
                        self.error = self.source_ui.begin_pending(&self.path).err();
                    }
                }
                if start || scheduled {
                    self.error = self.start(start).err();
                }
                if stop {
                    self.error = self.stop(false).err();
                }
                if duplicate {
                    self.new_task(true);
                }
                if delete && self.selected < self.config.tasks.len() {
                    self.config.tasks.remove(self.selected);
                    let next = self.selected.min(self.config.tasks.len().saturating_sub(1));
                    // The old row is gone; do not apply its date field to the following task.
                    self.selected = usize::MAX;
                    self.select(next);
                    self.error = self.save().err();
                }
                if retry && let Some(task) = self.config.tasks.get(self.selected) {
                    self.error = hf::reset(&self.path, &task.id, None).err();
                }
                if self.dirty() {
                    ui.weak(t("Unsaved changes"));
                } else if self.saved {
                    ui.weak(t("Saved"));
                }
            });
        });
    }
}

fn open_folder(path: &std::path::Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("output folder does not exist".into());
    }
    #[cfg(windows)]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program).arg(path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn folder(ui: &mut egui::Ui, value: &mut PathBuf, label: &str, language: Language) {
    ui.label(label);
    ui.horizontal(|ui| {
        let mut text = value.to_string_lossy().to_string();
        if ui.add(egui::TextEdit::singleline(&mut text).desired_width((ui.available_width() - 110.0).max(100.0))).changed() {
            *value = text.into();
        }
        if ui.button(language.t("Browse…")).clicked()
            && let Some(path) = rfd::FileDialog::new().set_title(label).pick_folder()
        {
            *value = path;
        }
    });
}

fn image_preparation(ui: &mut egui::Ui, settings: &mut pdfcraft_engine::hot_folder_preprocess::Settings, language: Language) {
    let t = |key| language.t(key);
    ui.collapsing(t("Page selection and image preparation"), |ui| {
        ui.label(t("Pages to process (empty = all; example: 1-3,5)"));
        ui.text_edit_singleline(&mut settings.pages);
        ui.weak(t("Only selected pages are included in the output."));
        ui.add(crate::widgets::checkbox(&mut settings.grayscale, t("Convert to grayscale")));
        ui.add(crate::widgets::checkbox(&mut settings.invert, t("Invert colors")));
        ui.add(crate::widgets::checkbox(&mut settings.median_filter, t("Reduce noise (median filter)")));
        ui.add(crate::widgets::checkbox(&mut settings.split_spreads, t("Split spreads into left and right pages")));
        ui.horizontal(|ui| {
            ui.label(t("Brightness"));
            ui.add(egui::DragValue::new(&mut settings.brightness).range(-100..=100));
            ui.label(t("Contrast"));
            ui.add(egui::DragValue::new(&mut settings.contrast).range(-100.0..=100.0));
        });
        let mut binary = settings.threshold.is_some();
        if ui.add(crate::widgets::checkbox(&mut binary, t("Convert to black and white"))).changed() {
            settings.threshold = binary.then_some(128);
        }
        if let Some(threshold) = &mut settings.threshold {
            ui.horizontal(|ui| {
                ui.label(t("Threshold"));
                ui.add(egui::DragValue::new(threshold).range(1..=254));
            });
        }
        if settings.changes_images() {
            ui.weak(t("Image changes rasterize pages at the chosen resolution. Documents with forms, links or annotations are refused."));
        }
    });
    ui.collapsing(t("Recognition areas (saved with task)"), |ui| {
        ui.weak(t("Coordinates are percentages from the top left. Page 0 applies to every prepared page. Visible content remains unchanged."));
        let mut remove = None;
        for (index, region) in settings.regions.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("Page"));
                    ui.add(egui::DragValue::new(&mut region.page).range(0..=10000));
                    ui.add(crate::widgets::checkbox(&mut region.exclude, t("Exclude this area")));
                    if ui.button(t("Remove area")).clicked() {
                        remove = Some(index);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (label, value) in ["Left (%)", "Top (%)", "Width (%)", "Height (%)"].into_iter().zip(region.rect.iter_mut()) {
                        let mut percent = *value * 100.0;
                        ui.label(t(label));
                        if ui.add(egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(0.5)).changed() {
                            *value = percent / 100.0;
                        }
                    }
                });
                ui.separator();
            });
        }
        if let Some(index) = remove {
            settings.regions.remove(index);
        }
        if ui.add_enabled(settings.regions.len() < 64, egui::Button::new(t("Add recognition area"))).clicked() {
            settings.regions.push(Default::default());
        }
    });
}

fn needs_ocr(task: &hf::Task) -> bool {
    !task.analysis_only && (task.settings.recognize || task.settings.auto_rotate || task.settings.deskew || task.settings.auto_crop)
}

fn output_steps(ui: &mut egui::Ui, plan: &mut output::OutputPlan, language: Language, analysis: bool) {
    let t = |key| language.t(key);
    ui.collapsing(t("Output saving steps"), |ui| {
        ui.weak(t("All outputs must be saved before originals are moved or deleted. Existing files are never overwritten."));
        let count = plan.steps.len();
        let mut remove = None;
        for (index, step) in plan.steps.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{} {}", t("Save"), index.saturating_add(1)));
                    ui.add_enabled_ui(!analysis, |ui| {
                        let shown = if analysis { output::Format::Analysis } else { step.format };
                        egui::ComboBox::from_id_salt("format").selected_text(t(shown.label())).show_ui(ui, |ui| {
                            for format in output::Format::ALL {
                                if ui.selectable_value(&mut step.format, *format, t(format.label())).changed()
                                    && !matches!(step.format, output::Format::Pdf | output::Format::ImagePdf)
                                {
                                    step.optimization.mode = output::optimization::Mode::Off;
                                }
                            }
                        });
                    });
                    if ui.add_enabled(count > 1, egui::Button::new(t("Remove step"))).clicked() {
                        remove = Some(index);
                    }
                });
                egui::ComboBox::from_id_salt("group").selected_text(t(grouping_label(step.grouping))).show_ui(ui, |ui| {
                    for grouping in [output::Grouping::File, output::Grouping::Folder, output::Grouping::All] {
                        ui.selectable_value(&mut step.grouping, grouping, t(grouping_label(grouping)));
                    }
                });
                let mut custom = step.folder.is_some();
                if ui.add(crate::widgets::checkbox(&mut custom, t("Use another output folder"))).changed() {
                    step.folder = custom.then(PathBuf::new);
                }
                if let Some(path) = &mut step.folder {
                    folder(ui, path, t("Output folder"), language);
                }
                ui.label(t("Filename template (without extension)"));
                ui.text_edit_singleline(&mut step.name_template);
                ui.weak("{name} · {folder} · {task} · {date} · {time} · {counter}");
                if !analysis && matches!(step.format, output::Format::Png | output::Format::Jpeg | output::Format::Tiff | output::Format::ImagePdf) {
                    ui.horizontal(|ui| {
                        ui.label(t("Resolution (dpi)"));
                        ui.add(egui::DragValue::new(&mut step.dpi).range(18.0..=1200.0));
                    });
                }
                if !analysis && step.format == output::Format::Jpeg {
                    ui.horizontal(|ui| {
                        ui.label(t("JPEG quality"));
                        ui.add(egui::DragValue::new(&mut step.jpeg_quality).range(1..=100));
                    });
                }
                if !analysis && matches!(step.format, output::Format::Pdf | output::Format::ImagePdf) {
                    pdf_optimization(ui, &mut step.optimization, language);
                }
                ui.separator();
            });
        }
        if let Some(index) = remove {
            plan.steps.remove(index);
        }
        if ui.add_enabled(plan.steps.len() < 16, egui::Button::new(t("Add saving step"))).clicked() {
            plan.steps.push(output::SaveStep::default());
        }
    });
}

fn pdf_optimization(ui: &mut egui::Ui, settings: &mut output::optimization::Settings, language: Language) {
    use output::optimization::Mode;
    let t = |key| language.t(key);
    let label = |mode| match mode {
        Mode::Off => t("Off"),
        Mode::Lossless => t("Lossless PDF optimization"),
        Mode::Reduce => t("Reduce PDF size (150 ppi, JPEG 60)"),
        Mode::Custom => t("Custom PDF optimization"),
    };
    ui.label(t("PDF optimization"));
    egui::ComboBox::from_id_salt("pdf-optimization").selected_text(label(settings.mode)).show_ui(ui, |ui| {
        for mode in [Mode::Off, Mode::Lossless, Mode::Reduce, Mode::Custom] {
            ui.selectable_value(&mut settings.mode, mode, label(mode));
        }
    });
    if settings.enabled() {
        ui.weak(t("Only a smaller PDF replaces the processed copy. Signed PDFs are refused. Original-file actions still apply."));
        match settings.mode {
            Mode::Lossless => {
                ui.weak(t("Keeps image quality, text, tags and forms. OCR and other task processing are separate settings."));
            }
            Mode::Reduce | Mode::Custom => {
                ui.weak(t("Downsampling and JPEG compression can lose image detail. Review the output quality."));
            }
            Mode::Off => {}
        }
    }
    if settings.mode == Mode::Custom {
        optimization_images(ui, &mut settings.color, language, "color", t("Color images"));
        optimization_images(ui, &mut settings.gray, language, "gray", t("Grayscale images"));
    }
}

fn optimization_images(ui: &mut egui::Ui, settings: &mut output::optimization::Images, language: Language, id: &str, title: &str) {
    use output::optimization::Compression;
    let t = |key| language.t(key);
    ui.push_id(id, |ui| {
        ui.label(title);
        ui.add(crate::widgets::checkbox(&mut settings.downsample, t("Downsample images")));
        ui.add_enabled_ui(settings.downsample, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(t("Target resolution (ppi)"));
                ui.add(egui::DragValue::new(&mut settings.ppi).range(9.0..=2400.0));
                ui.label(t("Only above (ppi)"));
                ui.add(egui::DragValue::new(&mut settings.above_ppi).range(9.0..=2400.0));
            });
        });
        let label = |compression| match compression {
            Compression::Retain => t("Retain existing compression"),
            Compression::Zip => t("ZIP (lossless encoding)"),
            Compression::Jpeg => "JPEG",
        };
        egui::ComboBox::from_id_salt("compression").selected_text(label(settings.compression)).show_ui(ui, |ui| {
            for compression in [Compression::Retain, Compression::Zip, Compression::Jpeg] {
                ui.selectable_value(&mut settings.compression, compression, label(compression));
            }
        });
        if settings.compression == Compression::Jpeg {
            ui.horizontal_wrapped(|ui| {
                ui.label(t("JPEG quality"));
                ui.add(egui::DragValue::new(&mut settings.quality).range(1..=100));
            });
        }
    });
}

fn grouping_label(group: output::Grouping) -> &'static str {
    match group {
        output::Grouping::File => "One document per file",
        output::Grouping::Folder => "Combine files in each folder",
        output::Grouping::All => "Combine all files in this run",
    }
}

fn calendar(ui: &mut egui::Ui, schedule: &mut hf::Schedule, language: Language) {
    let t = |key| language.t(key);
    let mut kind = match schedule {
        hf::Schedule::Interval => 0,
        hf::Schedule::Daily { .. } => 1,
        hf::Schedule::Weekly { .. } => 2,
        hf::Schedule::Monthly { .. } => 3,
    };
    let old = kind;
    let names = ["At regular intervals", "Daily", "Weekly", "Monthly"];
    egui::ComboBox::from_id_salt("recurrence").selected_text(t(names.get(kind).copied().unwrap_or(names[0]))).show_ui(ui, |ui| {
        for (index, name) in names.iter().enumerate() {
            ui.selectable_value(&mut kind, index, t(name));
        }
    });
    if old != kind {
        *schedule = match kind {
            1 => hf::Schedule::Daily { hour: 8, minute: 0 },
            2 => hf::Schedule::Weekly { weekday: 0, hour: 8, minute: 0 },
            3 => hf::Schedule::Monthly { day: 1, hour: 8, minute: 0 },
            _ => hf::Schedule::Interval,
        };
    }
    if let hf::Schedule::Weekly { weekday, .. } = schedule {
        let weekdays = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
        egui::ComboBox::from_id_salt("weekday").selected_text(t(weekdays.get(*weekday as usize).copied().unwrap_or(weekdays[0]))).show_ui(ui, |ui| {
            for (index, name) in weekdays.iter().enumerate() {
                ui.selectable_value(weekday, index as u32, t(name));
            }
        });
    }
    if let hf::Schedule::Monthly { day, .. } = schedule {
        ui.horizontal(|ui| {
            ui.label(t("Day of month"));
            ui.add(egui::DragValue::new(day).range(1..=31));
        });
    }
    match schedule {
        hf::Schedule::Daily { hour, minute } | hf::Schedule::Weekly { hour, minute, .. } | hf::Schedule::Monthly { hour, minute, .. } => {
            ui.horizontal(|ui| {
                ui.label(t("Time"));
                ui.add(egui::DragValue::new(hour).range(0..=23));
                ui.label(":");
                ui.add(egui::DragValue::new(minute).range(0..=59));
            });
        }
        hf::Schedule::Interval => {}
    }
}

fn read_disk(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err("configuration exceeds 16 MiB".into());
            }
            Ok(Some(bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

impl eframe::App for HotFolderApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.allow_close && ctx.input(|i| i.viewport().close_requested()) && self.dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_prompt = true;
        }
        self.show(ui);
        if self.close_prompt {
            let lang = self.language;
            egui::Modal::new(egui::Id::new("unsaved-hot-folder")).show(&ctx, |ui| {
                ui.heading(lang.t("Unsaved changes"));
                ui.label(lang.t("Save your task settings before closing?"));
                ui.horizontal(|ui| {
                    if ui.button(lang.t("Save and close")).clicked() {
                        match self.save() {
                            Ok(()) => {
                                self.allow_close = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            Err(error) => self.error = Some(error),
                        }
                        self.close_prompt = false;
                    }
                    if ui.button(lang.t("Discard and close")).clicked() {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button(lang.t("Cancel")).clicked() {
                        self.close_prompt = false;
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn hot_folder_small_window_keeps_actions_tasks_service_and_errors_reachable_in_every_language() {
        for (language, (code, _)) in LANGUAGES.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let mut h = Harness::builder().with_size(egui::vec2(780.0, 560.0)).build_eframe(|cc| {
                install_fonts(&cc.egui_ctx);
                let mut app = HotFolderApp::new(dir.path().join("tasks.json"));
                app.language = Language(language);
                app.inline = true;
                app.error = None;
                for index in 0..32 {
                    app.new_task(false);
                    app.config.tasks[index].name = format!("Task {}", index + 1);
                    app.config.tasks[index].analysis_only = true;
                }
                app.select(0);
                app
            });
            h.run_steps(10);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(780.0, 560.0));
            let t = |key| Language(language).t(key);
            for key in ["Save", "Start now", "Start on schedule", "Stop", "Duplicate", "Delete task"] {
                assert!(viewport.contains_rect(h.get_by_label(t(key)).rect()), "{code}: {key} must fit the minimum window");
            }
            // The long task list must scroll to both its final row and the service controls.
            let last = format!("Task 32\n{}", t("Stopped"));
            h.get_by_label(&last).scroll_to_me();
            h.run_steps(10);
            assert!(viewport.contains_rect(h.get_by_label(&last).rect()), "{code}: final task must be reachable");
            h.get_by_label(t("Stop all")).scroll_to_me();
            h.run_steps(10);
            assert!(viewport.contains_rect(h.get_by_label(t("Stop all")).rect()), "{code}: Stop all must be reachable");
            h.get_by_label(t("System service")).scroll_to_me();
            h.run_steps(10);
            h.get_by_label(t("System service")).click();
            h.run_steps(10);
            h.get_by_label(t("Refresh status")).scroll_to_me();
            h.run_steps(10);
            assert!(viewport.contains_rect(h.get_by_label(t("Refresh status")).rect()), "{code}: service controls must be reachable");
            // An invalid input path must report the start error at the top, without a scroll.
            h.get_by_label(t("Start now")).click();
            h.run_steps(10);
            let error = h.state().error.clone().expect("unspecified task folders must fail validation");
            assert!(viewport.contains_rect(h.get_by_label(t("Error details")).rect()), "{code}: error heading must stay visible");
            assert!(viewport.intersects(h.get_by_label(&error).rect()), "{code}: failure detail must stay visible");
            if let Ok(shots) = std::env::var("PDFCRAFT_HOT_FOLDER_SMALL_SHOTS") {
                h.render().unwrap().save(std::path::Path::new(&shots).join(format!("hot-folder-{code}-small.png"))).unwrap();
            }
            let input = dir.path().join("in");
            let output = dir.path().join("out");
            std::fs::create_dir(&input).unwrap();
            std::fs::create_dir(&output).unwrap();
            for task in &mut h.state_mut().config.tasks {
                task.input = input.clone();
                task.output = output.clone();
            }
            h.get_by_label(t("Start now")).click();
            h.run_steps(10);
            assert!(h.state().error.is_none(), "{code}: {:?}", h.state().error);
            assert!(hf::load_config(&h.state().path).unwrap().tasks[0].enabled, "{code}: start must enable the selected task");
            h.get_by_label(t("Stop")).click();
            h.run_steps(10);
            assert!(h.state().error.is_none(), "{code}: {:?}", h.state().error);
            assert!(!hf::load_config(&h.state().path).unwrap().tasks[0].enabled, "{code}: stop must persist the cancellation");
        }
    }

    #[test]
    fn hot_folder_ui_preserves_settings_changed_by_another_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let mut app = HotFolderApp::new(path.clone());
        app.language = Language(0);
        app.new_task(false);
        for name in ["in", "out"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        app.config.tasks[0].input = dir.path().join("in");
        app.config.tasks[0].output = dir.path().join("out");
        assert!(app.dirty());
        hf::save_config(&path, &hf::Config::default(), None).unwrap();
        assert!(app.save().unwrap_err().contains("Reload"));
        assert!(hf::load_config(&path).unwrap().tasks.is_empty());
    }

    #[test]
    fn hot_folder_optimization_ui_persists_choices_and_clears_them_for_non_pdf_outputs() {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["in", "out"] {
            std::fs::create_dir(dir.path().join(folder)).unwrap();
        }
        let path = dir.path().join("tasks.json");
        let mut h = Harness::builder().with_size(egui::vec2(900.0, 780.0)).build_eframe(|cc| {
            install_fonts(&cc.egui_ctx);
            let mut app = HotFolderApp::new(path.clone());
            app.language = Language(0);
            app.inline = true;
            app.new_task(false);
            app.config.tasks[0].input = dir.path().join("in");
            app.config.tasks[0].output = dir.path().join("out");
            app
        });
        h.run_steps(10);
        h.get_by_label("Output saving steps").scroll_to_me();
        h.run_steps(10);
        h.get_by_label("Output saving steps").click();
        h.run_steps(10);
        h.get_by_value("Off").scroll_to_me();
        h.run_steps(10);
        h.get_by_value("Off").click();
        h.run_steps(3);
        h.get_by_label("Lossless PDF optimization").click();
        h.run_steps(10);
        h.get_by_label("Save").scroll_to_me();
        h.run_steps(20);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("Save").rect()), "Save must be visible before clicking");
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        let saved = hf::load_config(&path).unwrap();
        assert_eq!(saved.tasks[0].output_plan.steps[0].optimization.mode, output::optimization::Mode::Lossless);
        assert_eq!(saved.tasks[0].originals, hf::Originals::Keep);
        assert!(!saved.tasks[0].enabled);

        h.get_by_value("Lossless PDF optimization").scroll_to_me();
        h.run_steps(10);
        h.get_by_value("Lossless PDF optimization").click();
        h.run_steps(3);
        h.get_by_label("Custom PDF optimization").click();
        h.run_steps(10);
        for label in ["Color images", "Grayscale images"] {
            h.get_by_label(label).scroll_to_me();
            h.run_steps(10);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 780.0));
            assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{label} must be reachable");
        }
        h.get_by_value("PDF (processed document)").scroll_to_me();
        h.run_steps(10);
        h.get_by_value("PDF (processed document)").click();
        h.run_steps(3);
        h.get_by_label("Plain text (TXT)").click();
        h.run_steps(3);
        h.get_by_label("Save").scroll_to_me();
        h.run_steps(20);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("Save").rect()), "Save must be visible before clicking");
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        let saved = hf::load_config(&path).unwrap();
        assert_eq!(saved.tasks[0].output_plan.steps[0].format, output::Format::Txt);
        assert_eq!(saved.tasks[0].output_plan.steps[0].optimization.mode, output::optimization::Mode::Off);
    }

    #[test]
    fn hot_folder_ui_has_seven_languages_and_persists_a_stopped_task() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in");
        let output = dir.path().join("out");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        let path = dir.path().join("tasks.json");
        let mut h = Harness::builder().with_size(egui::vec2(1100.0, 1000.0)).build_eframe(|cc| {
            install_fonts(&cc.egui_ctx);
            let mut app = HotFolderApp::new(path.clone());
            app.language = Language(0);
            app.inline = true;
            app
        });
        h.run_steps(2);
        h.get_by_label("New task").click();
        h.run_steps(2);
        let task = h.state_mut().config.tasks.first_mut().unwrap();
        task.input = input;
        task.output = output;
        h.run_steps(3);
        h.get_by_label("Save").scroll_to_me();
        h.run_steps(20);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("Save").rect()), "Save must be visible before clicking");
        h.get_by_label("Save").click();
        h.run_steps(2);
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        let config = hf::load_config(&path).unwrap();
        assert_eq!(config.tasks.len(), 1);
        assert!(!config.tasks[0].enabled);
        assert_eq!(config.tasks[0].originals, hf::Originals::Keep);
        use pdfcraft_engine::hot_folder_sources::SourceConfig;
        for (source, label) in [
            (SourceConfig::Ftp(Default::default()), "Save password securely"),
            (SourceConfig::Outlook(Default::default()), "Sign in to Outlook"),
            (SourceConfig::Project(Default::default()), "Choose project…"),
        ] {
            h.state_mut().config.tasks[0].source = source;
            h.run_steps(2);
            h.get_by_label(label);
            assert!(!h.state().source_ui.has_pending());
        }
        h.state_mut().config.tasks[0].source = SourceConfig::Local;
        for index in 0..LANGUAGES.len() {
            h.state_mut().language = Language(index);
            h.run_steps(2);
            h.get_by_label(Language(index).t("Task name"));
        }
        if let Ok(path) = std::env::var("PDFCRAFT_HOT_FOLDER_SHOT") {
            h.state_mut().language = Language(1);
            h.run_steps(3);
            h.render().unwrap().save(path).unwrap();
        }
    }
}
