use super::App;
use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::{Edit, commands};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, rc::Rc};

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    recent: Vec<PathBuf>,
    #[serde(default)]
    pinned_folder: Option<PathBuf>,
    #[serde(default)]
    saved_signature: Option<String>,
    #[serde(default)]
    saved_initials: Option<String>,
    #[serde(default)]
    redact_text: String,
    #[serde(default)]
    redact_overlay: bool,
    #[serde(default)]
    sign_id_path: String,
}

impl App {
    pub(super) fn supports_native(id: &str) -> bool {
        matches!(
            id,
            "file.recents"
                | "file.pin_folder"
                | "file.clear_recent"
                | "edit.preferences"
                | "protect.unlock"
                | "view.theme.system"
                | "view.theme.light"
                | "view.theme.dark"
                | "view.theme"
                | "view.focus_page_input"
                | "view.layout.single"
                | "view.layout.two_up"
                | "view.continuous"
                | "view.page_gaps"
                | "view.layout.cover"
                | "view.fit_width_scrolling"
                | "view.fit_one_page"
                | "view.fit_visible"
                | "view.previous_view"
                | "view.next_view"
                | "view.read_mode"
                | "view.side_panel"
                | "view.tools_pane"
                | "ui.dock.reset"
                | "view.marquee_zoom"
                | "comment.note"
                | "comment.freetext"
                | "comment.highlight"
                | "comment.underline"
                | "comment.strikeout"
                | "comment.squiggly"
                | "comment.ink"
                | "comment.line"
                | "comment.arrow"
                | "comment.square"
                | "comment.circle"
                | "comment.polygon"
                | "comment.polyline"
                | "comment.cloud"
                | "comment.callout"
                | "comment.caret"
                | "comment.replace"
                | "comment.attach"
                | "comment.eraser"
                | "comment.redact"
                | "redact.mark"
                | "redact.apply"
                | "sign.fill.text"
                | "sign.fill.check"
                | "sign.fill.cross"
                | "sign.fill.dot"
                | "sign.fill.line"
                | "sign.fill.date"
                | "sign.fill.signature"
                | "sign.fill.initials"
                | "app.preferences"
                | "page.properties"
                | "sign.fill.signature.remove"
                | "sign.fill.initials.remove"
                | "help.shortcuts"
                | "help.discord"
                | "help.website"
                | "help.check_updates"
        )
    }

    pub(super) fn run_native(self: &Rc<Self>, id: &str) -> bool {
        match id {
            "file.recents" => self.show_recents(),
            "file.pin_folder" => self.pin_active_folder(),
            "file.clear_recent" => {
                let mut settings = read_settings();
                settings.recent.clear();
                let _ = write_settings(&settings);
                self.toast.add_toast(adw::Toast::new("Historial de recientes vaciado"));
            }
            "edit.preferences" => self.show_preferences(),
            "protect.unlock" => self.show_unlock(),
            "view.theme.system" => set_theme(adw::ColorScheme::Default),
            "view.theme.light" => set_theme(adw::ColorScheme::ForceLight),
            "view.theme.dark" => set_theme(adw::ColorScheme::ForceDark),
            "view.theme" | "app.preferences" => self.show_preferences(),
            "view.focus_page_input" => self.show_go_to_page(),
            "view.layout.single"
            | "view.layout.two_up"
            | "view.continuous"
            | "view.page_gaps"
            | "view.layout.cover"
            | "view.fit_width_scrolling"
            | "view.fit_one_page"
            | "view.fit_visible" => {
                if let Some(view) = self.active_view() {
                    view.apply_view_command(id, &self.automation);
                }
            }
            "view.previous_view" => {
                if let Some(view) = self.active_view() {
                    view.previous_view(&self.automation);
                }
            }
            "view.next_view" => {
                if let Some(view) = self.active_view() {
                    view.next_view(&self.automation);
                }
            }
            "view.read_mode" => {
                let enabled = !self.read_mode.get();
                self.read_mode.set(enabled);
                self.header.set_visible(!enabled);
                self.tab_bar.set_visible(!enabled);
                self.side_panel.set_visible(false);
                self.tools_panel.set_visible(false);
                self.with_active(|_, view| view.set_read_mode(enabled));
            }
            "view.side_panel" => self.side_panel.set_visible(!self.side_panel.is_visible()),
            "view.tools_pane" => self.tools_panel.set_visible(!self.tools_panel.is_visible()),
            "ui.dock.reset" => {
                self.side_panel.set_visible(false);
                self.tools_panel.set_visible(false);
            }
            "view.marquee_zoom" => self.with_active(|_, view| view.set_marquee(true)),
            "comment.attach" => self.choose_attachment(),
            "comment.redact" | "redact.mark" => {
                if !self.tools_panel.is_visible() {
                    self.tools_panel.set_visible(true);
                }
                self.with_active(|_, view| view.set_annotation_tool(Some("comment.redact".to_string())));
            }
            "redact.apply" => self.confirm_redact_apply(),
            "sign.fill.text"
            | "sign.fill.check"
            | "sign.fill.cross"
            | "sign.fill.dot"
            | "sign.fill.line"
            | "sign.fill.date"
            | "sign.fill.signature"
            | "sign.fill.initials" => {
                if !self.tools_panel.is_visible() {
                    self.tools_panel.set_visible(true);
                }
                self.with_active(|_, view| view.set_annotation_tool(Some(id.to_string())));
            }
            _ if id.starts_with("comment.") => {
                self.with_active(|_, view| view.set_annotation_tool(Some(id.to_string())));
            }
            "page.properties" => self.show_page_properties(),
            "sign.fill.signature.remove" => self.remove_saved_signature(false),
            "sign.fill.initials.remove" => self.remove_saved_signature(true),
            "help.shortcuts" => self.show_shortcuts(),
            "help.discord" => self.open_uri("https://discord.gg/ppuRWvQnNG"),
            "help.website" => self.open_uri("https://artcraft.software/pdfcraft"),
            "help.check_updates" => self.open_uri("https://github.com/storytold/pdfcraft/releases/latest"),
            _ => return false,
        }
        true
    }

    fn confirm_redact_apply(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let marks = self.automation.borrow().session().get(view.id).map_or(0, |d| d.redaction_marks());
        if marks == 0 {
            self.toast.add_toast(adw::Toast::new("No hay marcas de redacción que aplicar"));
            return;
        }
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let label = gtk::Label::new(Some(&format!(
            "Se eliminará de forma permanente el contenido cubierto por {marks} marcas de redacción. Puedes deshacerlo hasta guardar el documento."
        )));
        label.set_wrap(true);
        label.set_xalign(0.0);
        body.append(&label);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let cancel = gtk::Button::with_label("Cancelar");
        let apply = gtk::Button::with_label("Aplicar");
        apply.add_css_class("destructive-action");
        buttons.append(&cancel);
        buttons.append(&apply);
        buttons.set_halign(gtk::Align::End);
        body.append(&buttons);
        let window = window_with_content(self, "Aplicar redacción", &body, 520, 200);
        cancel.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.close()
        ));
        let app = self.clone();
        apply.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            app,
            move |_| {
                window.close();
                app.perform_redact_apply();
            }
        ));
        window.present();
    }

    pub(super) fn perform_redact_apply(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let id = view.id;
        let result = self.automation.borrow_mut().session_mut().apply(id, Edit::ApplyRedactions { pages: None });
        match result {
            Ok(()) => {
                if let Some(view) = self.active_view() {
                    view.refresh(&self.automation);
                }
                self.toast.add_toast(adw::Toast::new("Redacción aplicada"));
                self.update_chrome();
            }
            Err(error) => self.show_error(&format!("No se pudo aplicar la redacción: {error}")),
        }
    }

    fn choose_attachment(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Adjuntar un archivo como comentario");
        let app = self.clone();
        dialog.open(Some(&self.window), None::<&adw::gio::Cancellable>, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let Ok(metadata) = std::fs::metadata(&path) else { return };
            if metadata.len() > 16 * 1024 * 1024 {
                app.show_error("El adjunto supera el límite de 16 MiB");
                return;
            }
            match std::fs::read(&path) {
                Ok(data) => app.with_active(|_, view| {
                    view.set_attachment(path.file_name().map_or("adjunto".to_string(), |name| name.to_string_lossy().into_owned()), data);
                    view.set_annotation_tool(Some("comment.attach".to_string()));
                }),
                Err(error) => app.show_error(&format!("No se pudo leer el adjunto: {error}")),
            }
        });
    }

    fn show_recents(self: &Rc<Self>) {
        let settings = read_settings();
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        for path in &settings.recent {
            let row = adw::ActionRow::new();
            row.set_activatable(true);
            row.set_title(&glib::markup_escape_text(path.file_name().map_or("Documento", |name| name.to_str().unwrap_or("Documento"))));
            row.set_subtitle(&glib::markup_escape_text(&path.display().to_string()));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let path = path.clone();
            let app = self.clone();
            row.connect_activated(move |_| app.open_paths(vec![path.clone()]));
            list.append(&row);
        }
        if settings.recent.is_empty() {
            let row = adw::ActionRow::new();
            row.set_title("No hay documentos recientes");
            list.append(&row);
        }
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&list));
        present_window(self, "Documentos recientes", &scrolled, 680, 620);
    }

    fn pin_active_folder(&self) {
        let Some(folder) = self.active_view().and_then(|view| view.path()).and_then(|path| path.parent().map(PathBuf::from)) else {
            self.show_error("El documento activo no tiene una carpeta que fijar");
            return;
        };
        let mut settings = read_settings();
        settings.pinned_folder = Some(folder.clone());
        match write_settings(&settings) {
            Ok(()) => self.toast.add_toast(adw::Toast::new(&format!("Carpeta fijada: {}", folder.display()))),
            Err(error) => self.show_error(&format!("No se pudo guardar la carpeta: {error}")),
        }
    }

    fn show_preferences(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        let appearance = adw::PreferencesGroup::new();
        appearance.set_title("Apariencia");
        let theme = adw::ComboRow::new();
        theme.set_title("Tema");
        theme.set_model(Some(&gtk::StringList::new(&["Sistema", "Claro", "Oscuro"])));
        theme.set_selected(match adw::StyleManager::default().color_scheme() {
            adw::ColorScheme::ForceLight => 1,
            adw::ColorScheme::ForceDark => 2,
            _ => 0,
        });
        theme.connect_selected_notify(|row| {
            let scheme = match row.selected() {
                1 => adw::ColorScheme::ForceLight,
                2 => adw::ColorScheme::ForceDark,
                _ => adw::ColorScheme::Default,
            };
            set_theme(scheme);
        });
        appearance.add(&theme);
        page.add(&appearance);

        let documents = adw::PreferencesGroup::new();
        documents.set_title("Documentos");
        let settings = read_settings();
        let pinned = adw::ActionRow::new();
        pinned.set_title("Carpeta de apertura");
        pinned.set_subtitle(&glib::markup_escape_text(
            &settings.pinned_folder.as_ref().map_or("Carpeta predeterminada del sistema".to_string(), |path| path.display().to_string()),
        ));
        documents.add(&pinned);
        let clear = adw::ActionRow::new();
        clear.set_title("Limpiar documentos recientes");
        clear.set_activatable(true);
        clear.connect_activated(|_| {
            let mut settings = read_settings();
            settings.recent.clear();
            let _ = write_settings(&settings);
        });
        documents.add(&clear);
        page.add(&documents);
        present_window(self, "Preferencias", &page, 680, 620);
    }

    fn show_unlock(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let password = adw::PasswordEntryRow::new();
        password.set_title("Contraseña del documento");
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.append(&password);
        let unlock = gtk::Button::with_label("Desbloquear");
        unlock.add_css_class("suggested-action");
        unlock.set_margin_top(18);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&list);
        content.append(&unlock);
        let window = window_with_content(self, "Desbloquear PDF", &content, 480, 260);
        let app = self.clone();
        let target = window.clone();
        unlock.connect_clicked(move |_| {
            let result = app.automation.borrow_mut().session_mut().unlock(view.id, &password.text());
            match result {
                Ok(()) => {
                    view.refresh(&app.automation);
                    app.update_chrome();
                    app.toast.add_toast(adw::Toast::new("Documento desbloqueado"));
                    target.close();
                }
                Err(error) => app.show_error(&format!("No se pudo desbloquear: {error}")),
            }
        });
        window.present();
    }

    fn show_go_to_page(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let count = view.page_count();
        let page = adw::SpinRow::with_range(1.0, count as f64, 1.0);
        page.set_title("Número de página");
        page.set_value((view.current() + 1) as f64);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.append(&page);
        let go = gtk::Button::with_label("Ir a la página");
        go.add_css_class("suggested-action");
        go.set_margin_top(18);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&list);
        content.append(&go);
        let window = window_with_content(self, "Ir a página", &content, 440, 240);
        let target = window.clone();
        let app = self.clone();
        go.connect_clicked(move |_| {
            view.go_to(page.value() as usize - 1, &app.automation);
            target.close();
        });
        window.present();
    }

    fn show_page_properties(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let info = view.info.borrow();
        let Some(page) = info.pages.get(view.current()) else { return };
        let group = adw::PreferencesGroup::new();
        group.set_title(&format!("Página {} de {}", view.current() + 1, info.pages.len()));
        for (title, value) in [
            ("Etiqueta", page.label.clone()),
            ("Tamaño", format!("{:.2} x {:.2} pt", page.width, page.height)),
            ("Rotación", format!("{}°", page.rotation)),
            ("Recorte", format!("{:.2} x {:.2} pt", page.crop[2] - page.crop[0], page.crop[3] - page.crop[1])),
        ] {
            let row = adw::ActionRow::new();
            row.set_title(title);
            row.add_suffix(&gtk::Label::new(Some(&value)));
            group.add(&row);
        }
        drop(info);
        let rotate = gtk::Button::with_label("Rotar esta página 90°");
        rotate.add_css_class("suggested-action");
        rotate.set_margin_top(18);
        let app = self.clone();
        rotate.connect_clicked(move |_| app.run_registered("page.rotate"));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&group);
        content.append(&rotate);
        present_window(self, "Propiedades de página", &content, 560, 460);
    }

    fn remove_saved_signature(&self, initials: bool) {
        let mut settings = read_settings();
        if initials {
            settings.saved_initials = None;
        } else {
            settings.saved_signature = None;
        }
        match write_settings(&settings) {
            Ok(()) => self.toast.add_toast(adw::Toast::new(if initials { "Iniciales guardadas eliminadas" } else { "Firma guardada eliminada" })),
            Err(error) => self.show_error(&format!("No se pudo guardar la configuración: {error}")),
        }
    }

    fn show_shortcuts(self: &Rc<Self>) {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        for command in commands::COMMANDS {
            let Some(shortcut) = command.shortcut else { continue };
            let row = adw::ActionRow::new();
            row.set_title(&glib::markup_escape_text(command.label));
            let accelerator = super::command_accelerator(shortcut).replace("<Ctrl>", "Ctrl+").replace("<Shift>", "Shift+").replace('>', "");
            row.add_suffix(&gtk::Label::new(Some(&accelerator)));
            list.append(&row);
        }
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&list));
        present_window(self, "Atajos de teclado", &scrolled, 720, 700);
    }
}

pub(super) fn record_recent(path: &std::path::Path) {
    let mut settings = read_settings();
    settings.recent.retain(|recent| recent != path);
    settings.recent.insert(0, path.to_path_buf());
    settings.recent.truncate(20);
    let _ = write_settings(&settings);
}

pub(super) fn pinned_folder() -> Option<PathBuf> {
    read_settings().pinned_folder
}

pub(super) fn recent_paths() -> Vec<PathBuf> {
    read_settings().recent
}

pub(crate) fn redact_defaults() -> (String, bool) {
    let settings = read_settings();
    (settings.redact_text, settings.redact_overlay)
}

pub(crate) fn saved_signature(initials: bool) -> Option<String> {
    let settings = read_settings();
    let saved = if initials { settings.saved_initials } else { settings.saved_signature };
    saved.filter(|text| !text.trim().is_empty())
}

pub(crate) fn save_saved_signature(initials: bool, text: &str) {
    let mut settings = read_settings();
    let saved = (!text.trim().is_empty()).then(|| text.trim().to_string());
    if initials {
        settings.saved_initials = saved;
    } else {
        settings.saved_signature = saved;
    }
    let _ = write_settings(&settings);
}

pub(crate) fn saved_sign_id() -> String {
    read_settings().sign_id_path
}

pub(crate) fn save_sign_id(path: &str) {
    if path.trim().is_empty() {
        return;
    }
    let mut settings = read_settings();
    settings.sign_id_path = path.trim().to_string();
    let _ = write_settings(&settings);
}

pub(super) fn save_redact_defaults(text: &str, overlay: bool) {
    let mut settings = read_settings();
    settings.redact_text = text.to_string();
    settings.redact_overlay = overlay;
    let _ = write_settings(&settings);
}

fn set_theme(scheme: adw::ColorScheme) {
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn present_window(app: &App, title: &str, content: &impl IsA<gtk::Widget>, width: i32, height: i32) {
    window_with_content(app, title, content, width, height).present();
}

fn window_with_content(app: &App, title: &str, content: &impl IsA<gtk::Widget>, width: i32, height: i32) -> adw::Window {
    let window = adw::Window::builder()
        .transient_for(&app.window)
        .application(&app.application)
        .modal(true)
        .default_width(width)
        .default_height(height)
        .build();
    window.set_title(Some(title));
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(content));
    window.set_content(Some(&toolbar));
    window
}

fn settings_path() -> PathBuf {
    glib::user_config_dir().join("pdfcraft-gtk").join("settings.json")
}

fn read_settings() -> Settings {
    fs::read(settings_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn write_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    let parent = path.parent().ok_or_else(|| "la configuración no tiene directorio".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}
