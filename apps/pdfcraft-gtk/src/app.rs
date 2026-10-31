mod actions;
mod automation;
mod command_aliases;
mod editing;
pub(crate) mod native;
mod palette;
mod panels;
pub(crate) mod signing;

use crate::{capture, document::DocumentView};
use adw::{gio, glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::Automation;
use pdfcraft_engine::{AttachmentSource, DocId, Edit, FieldInfoKind, FieldValue, SignatureStatus, commands};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

pub struct App {
    application: adw::Application,
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toast: adw::ToastOverlay,
    header: adw::HeaderBar,
    stack: gtk::Stack,
    side_panel: gtk::Frame,
    tools_panel: gtk::Frame,
    read_mode: Cell<bool>,
    tab_bar: adw::TabBar,
    tabs: adw::TabView,
    automation: Rc<RefCell<Automation>>,
    panels: Rc<panels::DocumentPanel>,
    views: RefCell<HashMap<DocId, Rc<DocumentView>>>,
    closed_tabs: RefCell<Vec<(String, Option<PathBuf>)>>,
}

pub fn startup(application: &adw::Application) {
    ensure(application);
}

pub fn activate(application: &adw::Application) {
    let app = ensure(application);
    app.window.present();
}

pub fn open(application: &adw::Application, files: &[gio::File], _hint: &str) {
    let app = ensure(application);
    let paths = files.iter().filter_map(|file| file.path()).collect::<Vec<_>>();
    app.open_paths(paths);
    app.window.present();
}

fn ensure(application: &adw::Application) -> Rc<App> {
    if let Some(app) = APP.with(|slot| slot.borrow().clone()) {
        return app;
    }
    let app = App::new(application);
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    app.start_polling();
    app.schedule_qa();
    app
}

impl App {
    fn new(application: &adw::Application) -> Rc<Self> {
        let title = adw::WindowTitle::new("PdfCraft", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let window = adw::ApplicationWindow::builder().application(application).default_width(1280).default_height(900).build();
        window.set_title(Some("PdfCraft"));

        let automation = Rc::new(RefCell::new(Automation::new()));
        let tabs = adw::TabView::new();
        tabs.set_hexpand(true);
        tabs.set_vexpand(true);
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tabs));
        tab_bar.set_autohide(true);

        let empty = adw::StatusPage::new();
        empty.set_icon_name(Some("document-open-symbolic"));
        empty.set_title("Abrir un PDF");
        empty.set_description(Some("Visualiza y edita documentos con el motor PdfCraft en una interfaz nativa de GNOME."));
        empty.set_vexpand(true);
        let empty_open = gtk::Button::with_label("Abrir PDF");
        empty_open.add_css_class("suggested-action");
        empty_open.add_css_class("pill");
        empty_open.set_halign(gtk::Align::Center);
        empty.set_child(Some(&empty_open));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&tabs, Some("document"));
        stack.set_visible_child_name("empty");

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        let panels = panels::DocumentPanel::new(&automation);
        panels.build_tools();
        let tools_panel = side_panel("Tools", "Herramientas de anotación", panels.tools_container());
        let right_panel = side_panel("Document", "Comentarios, marcadores, páginas, campos y firmas", panels.container());
        tools_panel.set_visible(false);
        right_panel.set_visible(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&tools_panel);
        content.append(&stack);
        content.append(&right_panel);
        toolbar.set_content(Some(&content));
        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&toolbar));
        window.set_content(Some(&toast));

        let app = Rc::new(Self {
            application: application.clone(),
            window,
            title,
            toast,
            header: header.clone(),
            stack,
            side_panel: right_panel,
            tools_panel,
            read_mode: Cell::new(false),
            tab_bar,
            tabs,
            automation,
            panels,
            views: RefCell::new(HashMap::new()),
            closed_tabs: RefCell::new(Vec::new()),
        });

        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.set_tooltip_text(Some("Abrir PDF"));
        header.pack_start(&open_button);
        let menu_bar = gtk::PopoverMenuBar::from_model(Some(&app.menu()));
        header.pack_start(&menu_bar);

        open_button.connect_clicked(glib::clone!(
            #[weak]
            app,
            move |_| app.open_dialog()
        ));
        empty_open.connect_clicked(glib::clone!(
            #[weak]
            app,
            move |_| app.open_dialog()
        ));
        app.install_actions();
        app.install_document_actions();
        app.install_registry_actions();
        app.connect_tabs();
        app.install_css();
        app
    }

    fn menu(&self) -> gio::Menu {
        let menu = gio::Menu::new();
        for name in ["File", "Edit", "View", "Pages"] {
            let submenu = gio::Menu::new();
            for command in commands::COMMANDS.iter().filter(|command| command.menu == Some(name)) {
                submenu.append_item(&command_item(command));
            }
            if name == "View" {
                let navigation = gio::Menu::new();
                navigation.append(Some("Previous page"), Some("win.previous-page"));
                navigation.append(Some("Next page"), Some("win.next-page"));
                navigation.append(Some("Zoom out"), Some("win.zoom-out"));
                navigation.append(Some("Zoom in"), Some("win.zoom-in"));
                navigation.append(Some("Fit page"), Some("win.fit-page"));
                navigation.append(Some("Actual size"), Some("win.actual-size"));
                submenu.append_section(None, &navigation);
            }
            menu.append_submenu(Some(name), &submenu);
        }
        let window_menu = gio::Menu::new();
        window_menu.append(Some("Reopen closed tab"), Some("win.reopen-closed-tab"));
        window_menu.append(Some("Reveal in file manager"), Some("win.reveal-file"));
        menu.append_submenu(Some("Window"), &window_menu);
        let help_submenu = gio::Menu::new();
        for command in commands::COMMANDS.iter().filter(|command| command.menu == Some("Help")) {
            help_submenu.append_item(&command_item(command));
        }
        menu.append_submenu(Some("Help"), &help_submenu);
        menu
    }

    fn install_registry_actions(self: &Rc<Self>) {
        for command in commands::COMMANDS {
            let name = command_action(command.id);
            let action = gio::SimpleAction::new(&name, None);
            let app = self.clone();
            let id = command.id;
            action.connect_activate(move |_, _| app.run_registered(id));
            self.window.add_action(&action);
            if let Some(shortcut) = command.shortcut {
                let accelerator = command_accelerator(shortcut);
                self.application.set_accels_for_action(&format!("win.{name}"), &[&accelerator]);
            }
        }
    }

    fn install_actions(self: &Rc<Self>) {
        self.action("open", |app| app.open_dialog());
        self.action("close-tab", |app| app.close_active());
        self.action("next-page", |app| app.with_active(|app, view| view.next(&app.automation)));
        self.action("previous-page", |app| app.with_active(|app, view| view.previous(&app.automation)));
        self.action("zoom-in", |app| app.with_active(|app, view| view.zoom_in(&app.automation)));
        self.action("zoom-out", |app| app.with_active(|app, view| view.zoom_out(&app.automation)));
        self.action("fit-page", |app| app.with_active(|app, view| view.fit_page(&app.automation)));
        self.action("actual-size", |app| app.with_active(|app, view| view.actual_size(&app.automation)));
        self.action("about", |app| app.about());
        self.action("reopen-closed-tab", |app| app.reopen_closed_tab());
        self.action("reveal-file", |app| app.reveal_active_file());
        self.application.set_accels_for_action("win.next-page", &["Page_Down"]);
        self.application.set_accels_for_action("win.previous-page", &["Page_Up"]);
        self.application.set_accels_for_action("win.zoom-in", &["<Ctrl>plus", "<Ctrl>equal"]);
        self.application.set_accels_for_action("win.zoom-out", &["<Ctrl>minus"]);
        self.application.set_accels_for_action("win.fit-page", &["<Ctrl>0"]);
    }

    fn action(self: &Rc<Self>, name: &str, callback: impl Fn(&Rc<Self>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let app = self.clone();
        action.connect_activate(move |_, _| callback(&app));
        self.window.add_action(&action);
    }

    fn connect_tabs(self: &Rc<Self>) {
        self.tabs.connect_n_pages_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.update_chrome()
        ));
        self.tabs.connect_selected_page_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.update_chrome();
                if let Some(view) = this.active_view() {
                    view.request_render(&this.automation);
                }
            }
        ));
        self.tabs.connect_close_page(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, page| {
                let child = page.child();
                let found = this.views.borrow().iter().find(|(_, view)| view.root.clone().upcast::<gtk::Widget>() == child).map(|(id, _)| *id);
                if let Some(id) = found {
                    if let Some(view) = this.views.borrow().get(&id).cloned() {
                        let mut history = this.closed_tabs.borrow_mut();
                        history.push((view.name(), view.path()));
                        if history.len() > 10 {
                            let extra = history.len() - 10;
                            history.drain(0..extra);
                        }
                    }
                    this.views.borrow_mut().remove(&id);
                    this.automation.borrow_mut().session_mut().close(id);
                }
                glib::Propagation::Proceed
            }
        ));
    }

    fn install_css(&self) {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(".pdf-page { background: white; box-shadow: 0 1px 5px rgba(0, 0, 0, 0.28); }");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    }

    fn start_polling(self: &Rc<Self>) {
        let app = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(app) = app.upgrade() else { return glib::ControlFlow::Break };
            let views = app.views.borrow().values().cloned().collect::<Vec<_>>();
            for view in views {
                view.poll(&app.automation);
            }
            app.panels.poll_thumbs();
            glib::ControlFlow::Continue
        });
    }

    fn update_chrome(&self) {
        let has_document = self.tabs.n_pages() > 0;
        self.stack.set_visible_child_name(if has_document { "document" } else { "empty" });
        self.tab_bar.set_visible(has_document);
        if let Some(view) = self.active_view() {
            self.title.set_title(&view.name());
            self.title.set_subtitle(view.path().as_ref().map_or("", |path| path.to_str().unwrap_or("")));
        } else {
            self.title.set_title("PdfCraft");
            self.title.set_subtitle("");
        }
        let active_id = self.active_view().map(|view| view.id);
        self.panels.update(self.active_view().as_ref());
        let automation = self.automation.borrow();
        for command in commands::COMMANDS {
            if let Some(action) =
                self.window.lookup_action(&command_action(command.id)).and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            {
                action.set_enabled(commands::is_enabled(command, automation.session(), active_id));
            }
        }
    }

    fn active_view(&self) -> Option<Rc<DocumentView>> {
        let page = self.tabs.selected_page()?;
        let child = page.child();
        self.views.borrow().values().find(|view| view.root.clone().upcast::<gtk::Widget>() == child).cloned()
    }

    fn with_active(&self, callback: impl FnOnce(&Self, Rc<DocumentView>)) {
        if let Some(view) = self.active_view() {
            callback(self, view);
        }
    }

    fn close_active(&self) {
        if let Some(page) = self.tabs.selected_page() {
            self.tabs.close_page(&page);
        }
    }

    fn reopen_closed_tab(self: &Rc<Self>) {
        let entry = self.closed_tabs.borrow_mut().pop();
        match entry {
            Some((_, Some(path))) => {
                if path.exists() {
                    self.open_paths(vec![path]);
                } else {
                    self.show_error("El fichero de la pestaña cerrada ya no existe");
                }
            }
            Some((_, None)) => self.show_error("La pestaña cerrada no guarda fichero en disco"),
            None => self.show_error("No hay pestañas cerradas"),
        }
    }

    fn reveal_active_file(&self) {
        let Some(path) = self.active_view().and_then(|view| view.path()) else {
            self.show_error("El documento activo no está guardado en disco");
            return;
        };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
        let window = self.window.clone();
        launcher.open_containing_folder(Some(&window), None::<&gio::Cancellable>, move |result| {
            if let Err(error) = result {
                eprintln!("[reveal] {error}");
            }
        });
    }

    fn open_dialog(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Abrir PDF");
        dialog.set_modal(true);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Documentos PDF"));
        filter.add_mime_type("application/pdf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));
        if let Some(folder) = native::pinned_folder() {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        let app = self.clone();
        dialog.open_multiple(Some(&self.window), None::<&gio::Cancellable>, move |result| match result {
            Ok(model) => {
                let mut paths = Vec::new();
                for index in 0..model.n_items() {
                    if let Some(file) = model.item(index).and_then(|item| item.downcast::<gio::File>().ok())
                        && let Some(path) = file.path()
                    {
                        paths.push(path);
                    }
                }
                app.open_paths(paths);
            }
            Err(error) if error.kind::<gtk::DialogError>() == Some(gtk::DialogError::Dismissed) => {}
            Err(error) => app.show_error(&format!("No se pudo abrir el selector: {error}")),
        });
    }

    fn open_paths(self: &Rc<Self>, paths: Vec<PathBuf>) {
        for path in paths {
            let app = self.clone();
            glib::spawn_future_local(async move {
                let read_path = path.clone();
                let bytes = match gio::spawn_blocking(move || std::fs::read(read_path)).await {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => {
                        app.show_error(&format!("No se pudo leer {}: {error}", path.display()));
                        return;
                    }
                    Err(_) => {
                        app.show_error("La tarea de lectura falló");
                        return;
                    }
                };
                app.open_bytes(path, bytes);
            });
        }
    }

    fn open_bytes(self: &Rc<Self>, path: PathBuf, bytes: Vec<u8>) {
        let name = path.file_name().map_or_else(|| "Documento.pdf".into(), |name| name.to_string_lossy().into_owned());
        native::record_recent(&path);
        self.open_automation_document(name, Some(path), Arc::new(bytes), false);
    }

    fn open_created(self: &Rc<Self>, name: String, bytes: Arc<Vec<u8>>) {
        self.open_automation_document(name, None, bytes, true);
    }

    fn open_automation_document(self: &Rc<Self>, name: String, path: Option<PathBuf>, bytes: Arc<Vec<u8>>, new_document: bool) {
        let opened = if new_document {
            self.automation.borrow_mut().session_mut().open_new(name.clone(), bytes)
        } else {
            self.automation.borrow_mut().session_mut().open(name.clone(), path.as_ref().map(|path| path.to_string_lossy().into_owned()), bytes, None)
        };
        let id = match opened {
            Ok(id) => id,
            Err(error) => {
                self.show_error(&format!("No se pudo abrir {name}: {error}"));
                return;
            }
        };
        let info = {
            let automation = self.automation.borrow();
            automation.session().get(id).map(|document| document.info.clone())
        };
        let Some(info) = info else {
            self.show_error("El documento no quedó registrado en la sesión");
            return;
        };
        let view = DocumentView::new(id, name, path.clone(), info, self.automation.clone());
        let page = self.tabs.append(&view.root);
        page.set_title(&view.name());
        if let Some(path) = path {
            page.set_tooltip(&path.to_string_lossy());
        }
        self.views.borrow_mut().insert(id, view.clone());
        self.connect_document(&view);
        self.tabs.set_selected_page(&page);
        view.go_to(0, &self.automation);
        self.update_chrome();
    }

    fn connect_document(self: &Rc<Self>, view: &Rc<DocumentView>) {
        let id = view.id;
        let app = Rc::downgrade(self);
        view.next_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.next(&app.automation));
            }
        });
        let app = Rc::downgrade(self);
        view.prev_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.previous(&app.automation));
            }
        });
        let app = Rc::downgrade(self);
        view.zoom_in_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.zoom_in(&app.automation));
            }
        });
        let app = Rc::downgrade(self);
        view.zoom_out_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.zoom_out(&app.automation));
            }
        });
        let app = Rc::downgrade(self);
        view.fit_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.fit_page(&app.automation));
            }
        });
        let app = Rc::downgrade(self);
        view.actual_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.with_document(id, |app, view| view.actual_size(&app.automation));
            }
        });
    }

    fn with_document(&self, id: DocId, callback: impl FnOnce(&Self, Rc<DocumentView>)) {
        if let Some(view) = self.views.borrow().get(&id).cloned() {
            callback(self, view);
        }
    }

    fn show_error(&self, message: &str) {
        eprintln!("{message}");
        self.toast.add_toast(adw::Toast::new(message));
    }

    fn about(&self) {
        let about = adw::AboutWindow::builder()
            .transient_for(&self.window)
            .application_name("PdfCraft")
            .version(env!("CARGO_PKG_VERSION"))
            .developer_name("Storytold y colaboradores")
            .website("https://github.com/storytold/pdfcraft")
            .issue_url("https://github.com/storytold/pdfcraft/issues")
            .license_type(gtk::License::Agpl30)
            .build();
        about.present();
    }

    fn schedule_qa(self: &Rc<Self>) {
        let Ok(mode) = std::env::var("PDFCRAFT_GTK_QA") else { return };
        if mode == "coverage" {
            self.schedule_coverage_qa();
            return;
        }
        let path = std::env::var_os("PDFCRAFT_GTK_QA_FILE").map_or_else(|| PathBuf::from("crates/ui-egui/tests/data/form.pdf"), PathBuf::from);
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(200), move || app.open_paths(vec![path]));
        if mode == "smoke" {
            let app = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(3500), move || {
                let Some(view) = app.active_view() else {
                    capture::mark(false, "smoke", &app.application);
                    return;
                };
                let path = capture::path("smoke");
                let has = view.has_page();
                capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                    capture::mark(has && captured.is_ok(), "smoke", &app.application);
                });
            });
        } else if mode == "viewer" {
            self.schedule_viewer_qa();
        } else if mode == "document" {
            self.schedule_document_qa();
        } else if mode == "palette" {
            let app = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(1800), move || app.show_palette());
        } else if mode == "automation" || mode == "form" {
            let app = self.clone();
            let command = if mode == "form" { "export.text" } else { "file.properties" };
            glib::timeout_add_local_once(Duration::from_millis(1800), move || app.run_registered(command));
        } else if mode == "reconcile" {
            self.schedule_reconcile_qa();
        } else if mode == "editing" {
            self.schedule_editing_qa();
        } else if mode == "screens" {
            self.schedule_screens_qa();
        } else if mode == "organizer" {
            self.schedule_organizer_qa();
        } else if mode == "panels-edit" {
            self.schedule_panels_edit_qa();
        } else if mode == "text-tools" {
            self.schedule_text_tools_qa();
        } else if mode == "redact" {
            self.schedule_redact_qa();
        } else if mode == "recents-open" {
            self.schedule_recents_open_qa();
        } else if mode == "fill-sign" {
            self.schedule_fill_sign_qa();
        } else if mode == "sign" {
            self.schedule_sign_qa();
        } else if mode == "page-ops" {
            self.schedule_page_ops_qa();
        } else if mode == "export-ops" {
            self.schedule_export_ops_qa();
        } else if mode == "parity-ops" {
            self.schedule_parity_ops_qa();
        }
    }

    fn schedule_recents_open_qa(self: &Rc<Self>) {
        let fixture =
            std::env::var_os("PDFCRAFT_GTK_QA_FILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("crates/ui-egui/tests/data/form.pdf"));
        let fixture = std::path::absolute(&fixture).unwrap_or(fixture);
        let app = self.clone();
        let fixture_open = fixture.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            app.open_paths(vec![fixture_open]);
        });
        let app = self.clone();
        let fixture_find = fixture.clone();
        let captured_prev = Rc::new(std::cell::Cell::new(false));
        let captured_slot = captured_prev.clone();
        glib::timeout_add_local_once(Duration::from_millis(3500), move || {
            let path = capture::path("recents-open");
            // Captura asíncrona; al terminar, abrir el reciente por el mismo camino que la fila.
            capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                let ok = captured.is_ok();
                captured_slot.set(ok);
                eprintln!("[DEBUG-recents-open] captura antes de reabrir={ok}");
                match crate::app::native::recent_paths().into_iter().find(|path| path == &fixture_find) {
                    Some(recent) => app.open_recent_path(&recent),
                    None => {
                        eprintln!("[DEBUG-recents-open] reciente no registrado");
                        capture::mark(false, "recents-open", &app.application);
                    }
                }
            });
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4400), move || {
            let opened = app.active_view().is_some_and(|view| view.path().as_deref() == Some(fixture.as_path()));
            eprintln!("[DEBUG-recents-open] abierta={opened} captured={}", captured_prev.get());
            capture::mark(opened && captured_prev.get(), "recents-open", &app.application);
        });
    }

    fn schedule_redact_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let created = app.automation.borrow().session().create_from_text("Redacción QA", "Confidencial 123");
            match created {
                Ok(bytes) => app.open_created("redact-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("redact: fixture fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1300), move || {
            app.run_native("redact.mark");
            let tool_ok = app.active_view().is_some_and(|view| view.annotation_tool_is("comment.redact"));
            eprintln!("[DEBUG-redact] herramienta redactar activa={tool_ok}");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1700), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "redact", &app.application);
                return;
            };
            // Mismo camino del motor que el arrastre: AddAnnotation con Shape::Redact.
            let marked = app.automation.borrow_mut().call("redact_mark", &serde_json::json!({ "doc": view.id.0, "find": "Confidencial" }));
            if let Err(error) = marked {
                eprintln!("redact: redact_mark fallo: {error}");
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2400), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "redact", &app.application);
                return;
            };
            let marks = app.automation.borrow().session().get(view.id).map_or(0, |d| d.redaction_marks());
            eprintln!("[DEBUG-redact] antes marks={marks}");
            if marks == 0 {
                capture::mark(false, "redact", &app.application);
                return;
            }
            app.perform_redact_apply();
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(3400), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "redact", &app.application);
                return;
            };
            let document = app
                .automation
                .borrow()
                .session()
                .get(view.id)
                .map(|document| (document.redaction_marks(), document.text_lines(0).iter().all(|line| !line.text.contains("Confidencial"))));
            let (marks, hidden) = document.map_or((0, false), |(marks, hidden)| (marks, hidden));
            let path = capture::path("redact-apply");
            eprintln!("[DEBUG-redact] despues marks={marks} texto_oculto={hidden}");
            capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                eprintln!("[DEBUG-redact] captured={}", captured.is_ok());
                capture::mark(marks == 0 && hidden && captured.is_ok(), "redact", &app.application);
            });
        });
    }

    fn schedule_text_tools_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let created = app.automation.borrow().session().create_from_text("Búsqueda QA", "A");
            match created {
                Ok(bytes) => app.open_created("text-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("text-tools: fixture fallo: {error}"),
            }
        });
        let clipboard_text = Rc::new(RefCell::new(None::<String>));
        let app = self.clone();
        let clipboard_slot = clipboard_text.clone();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            app.run_editing("edit.copy");
            app.window.clipboard().read_text_async(adw::gio::Cancellable::NONE, move |result| {
                if let Ok(Some(text)) = result {
                    *clipboard_slot.borrow_mut() = Some(text.to_string());
                }
            });
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "text-tools", &app.application);
                return;
            };
            let found = app.automation.borrow_mut().call("text_find", &serde_json::json!({ "doc": view.id.0, "query": "A" }));
            match found {
                Ok(contents) => {
                    app.show_automation_results("Búsqueda: A", contents);
                    app.update_chrome();
                }
                Err(error) => eprintln!("text-tools: text_find fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2600), move || {
            let clipboard_ok = clipboard_text.borrow().as_deref().is_some_and(|text| text.contains('A'));
            let results = automation::LAST_RESULTS.with_borrow(Clone::clone);
            let Some(results) = results else {
                eprintln!("text-tools: sin ventana de resultados clipboard_ok={clipboard_ok}");
                capture::mark(false, "text-tools", &app.application);
                return;
            };
            let path = capture::path("text-tools");
            let captured = capture::window_png(&results, &path).is_ok();
            eprintln!(
                "text-tools clipboard_ok={clipboard_ok} title={:?} size={}x{} captured={captured}",
                results.title(),
                results.width(),
                results.height()
            );
            results.close();
            capture::mark(clipboard_ok && captured, "text-tools", &app.application);
        });
    }

    fn schedule_editing_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1800), move || {
            app.run_editing("page.copy");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2200), move || {
            app.run_editing("page.paste");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "editing", &app.application);
                return;
            };
            let pages = view.page_count();
            let clip = App::page_clip();
            let (from, copied) = clip.map_or((DocId(0), 0), |(id, count)| (id, count));
            eprintln!("editing pages={pages} clip_from={from:?} copied={copied}");
            if pages != 3 || copied != 1 || from != view.id {
                capture::mark(false, "editing", &app.application);
                return;
            }
            let texture_ok = Rc::new(Cell::new(view.current_texture().is_some()));
            eprintln!("editing texture_before_annotations={}", texture_ok.get());
            app.run_editing("edit.snapshot");
            let index = app.automation.borrow().session().get(view.id).map_or(0, |document| document.info.annotations.len());
            let shape = pdfcraft_engine::Shape::Note { at: [120.0, 160.0], icon: pdfcraft_engine::NoteIcon::Comment };
            let style = pdfcraft_engine::Style::default_for(&shape);
            app.apply_edit(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page: view.current(),
                shape,
                style,
                contents: "QA".to_string(),
                author: "pdfcraft-gtk".to_string(),
            }));
            let annotations = app.automation.borrow().session().get(view.id).map(|document| document.info.annotations.clone()).unwrap_or_default();
            eprintln!("editing annotations {index}->{}", annotations.len());
            let Some(annotation) = annotations.iter().find(|annotation| annotation.page == view.current()) else {
                capture::mark(false, "editing", &app.application);
                return;
            };
            let target = annotation.index;
            if annotations.len() != index + 1 {
                capture::mark(false, "editing", &app.application);
                return;
            }
            let app = app.clone();
            let texture_ok = texture_ok.clone();
            glib::timeout_add_local_once(Duration::from_millis(3200), move || {
                let Some(view) = app.active_view() else {
                    capture::mark(false, "editing", &app.application);
                    return;
                };
                app.apply_edit(Edit::DeleteAnnotation { page: view.current(), index: target });
                let annotations =
                    app.automation.borrow().session().get(view.id).map(|document| document.info.annotations.clone()).unwrap_or_default();
                let annotation_pages: Vec<(usize, usize)> = annotations.iter().map(|annotation| (annotation.page, annotation.index)).collect();
                eprintln!(
                    "[DEBUG-editing] after_delete page={} index={} count={} details={annotation_pages:?}",
                    view.current(),
                    index,
                    annotations.len()
                );
                let app = app.clone();
                let texture_ok = texture_ok.clone();
                glib::timeout_add_local_once(Duration::from_millis(3800), move || {
                    let Some(view) = app.active_view() else {
                        capture::mark(false, "editing", &app.application);
                        return;
                    };
                    let erased = app.automation.borrow().session().get(view.id).map_or(usize::MAX, |document| document.info.annotations.len());
                    let child_info = app.window.child().map(|child| (child.width(), child.height(), child.is_mapped()));
                    let mut chain: Vec<String> = Vec::new();
                    let mut walker = Some(app.window.clone().upcast::<gtk::Widget>());
                    while let Some(widget) = walker {
                        chain.push(format!("{}({}x{}m{})", widget.type_().name(), widget.width(), widget.height(), widget.is_mapped() as u8));
                        walker = widget.first_child();
                    }
                    let parent_type = app.window.child().and_then(|child| child.parent()).map(|parent| parent.type_().name().to_string());
                    eprintln!(
                        "[DEBUG-editing] child={child_info:?} parent={parent_type:?} down={chain:?} windows={}",
                        app.application.windows().len()
                    );
                    let image = capture::path("editing");
                    capture::window_png_when_ready(app.window.clone(), &image, 8, move |result| {
                        eprintln!(
                            "editing erased={erased} texture={} captured={} err={:?} mapped={} visible={}",
                            texture_ok.get(),
                            result.is_ok(),
                            result.as_ref().err(),
                            app.window.is_mapped(),
                            app.window.is_visible()
                        );
                        capture::mark(erased == index && texture_ok.get() && result.is_ok(), "editing", &app.application);
                    });
                });
            });
        });
    }

    fn schedule_screens_qa(self: &Rc<Self>) {
        type ScreenStep = (&'static str, &'static str, fn(&Rc<App>), u64);
        let steps: [ScreenStep; 4] = [
            ("page.organize", "organizer", |app| app.run_editing("page.organize"), 2300),
            (
                "file.open_recent",
                "recents",
                |app| {
                    let seeded = app.active_view().and_then(|view| view.path().map(|path| path.to_path_buf()));
                    eprintln!("[DEBUG-screens] recent seeded={seeded:?}");
                    if let Some(path) = seeded {
                        native::record_recent(&path);
                    }
                    eprintln!("[DEBUG-screens] recent stored={:?}", native::recent_paths());
                    app.run_editing("file.open_recent");
                },
                3400,
            ),
            ("redact.properties", "redact", |app| app.run_editing("redact.properties"), 4600),
            (
                "screens",
                "panels",
                |app| {
                    app.run_native("view.side_panel");
                    app.run_native("view.tools_pane");
                    app.panels.select_section("Páginas");
                    if let Some(view) = app.active_view() {
                        let automation = app.automation.clone();
                        view.apply_view_command("layout.two_up", &automation);
                    }
                },
                5900,
            ),
        ];
        for (command, name, action, delay) in steps {
            let app = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                action(&app);
            });
            let app = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay + 400), move || {
                let target = app
                    .application
                    .windows()
                    .into_iter()
                    .filter_map(|window| window.downcast::<adw::Window>().ok())
                    .max_by_key(|window| i64::from(window.width()) * i64::from(window.height()));
                let captured: Result<(), String> = match target {
                    Some(window) => {
                        eprintln!(
                            "[DEBUG-screens] target {name} title={:?} size={}x{} main_visible={}",
                            window.title(),
                            window.width(),
                            window.height(),
                            app.window.is_visible()
                        );
                        let path = capture::path(name);
                        let window_cb = window.clone();
                        let app_retry = app.clone();
                        capture::window_png_when_ready(window, &path, 6, move |captured| match captured {
                            Ok(()) => {
                                window_cb.close();
                                // Sin mark: el gate "screens" lo valida el chequeo de
                                // 4200 ms sobre los cuatro PNG; marcar aquí cerraría
                                // la app tras la primera modal.
                                eprintln!("[DEBUG-screens] {name}: capturada");
                            }
                            Err(err) => {
                                eprintln!("[DEBUG-screens] {name}: captura falló: {err}");
                                capture::mark(false, name, &app_retry.application);
                            }
                        });
                        return;
                    }
                    None if command == "screens" => {
                        let path = capture::path(name);
                        let app_retry = app.clone();
                        capture::window_png_when_ready(app.window.clone(), &path, 6, move |result| {
                            if let Err(err) = &result {
                                eprintln!("[DEBUG-screens] {name}: captura principal falló: {err}");
                                capture::mark(false, name, &app_retry.application);
                            } else {
                                eprintln!("[DEBUG-screens] {name}: capturada");
                            }
                        });
                        return;
                    }
                    None => Err(format!("{command} no abrió una ventana")),
                };
                if captured.is_err() {
                    eprintln!("[DEBUG-screens] {name}: {captured:?}");
                    capture::mark(false, name, &app.application);
                }
            });
        }
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(7000), move || {
            let mut ok = true;
            for name in ["organizer", "recents", "redact", "panels"] {
                let path = capture::path(name);
                if !path.exists() || path.metadata().map_or(true, |metadata| metadata.len() < 1024) {
                    eprintln!("[DEBUG-screens] {name} ausente o vacío");
                    ok = false;
                }
            }
            capture::mark(ok, "screens", &app.application);
        });
    }

    /// Fill & Sign: activa herramientas sign.fill.*, coloca marca, fecha y firma guardada y
    /// verifica que las tres anotaciones quedan en el documento (mismo camino que el clic).
    fn schedule_fill_sign_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let created = app.automation.borrow().session().create_from_text("Relleno QA", "Casilla: ____");
            match created {
                Ok(bytes) => app.open_created("fill-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("fill-sign: fixture fallo: {error}"),
            }
        });
        let before = Rc::new(std::cell::Cell::new(0usize));
        let app = self.clone();
        let before_slot = before.clone();
        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
            app.run_registered("sign.fill.check");
            let Some(view) = app.active_view() else {
                capture::mark(false, "fill-sign", &app.application);
                return;
            };
            before_slot.set(view.info.borrow().annotations.len());
            eprintln!("[DEBUG-fill] tool_ok={} before={}", view.annotation_tool_is("sign.fill.check"), before_slot.get());
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2100), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "fill-sign", &app.application);
                return;
            };
            view.place_fill_mark(view.current(), pdfcraft_engine::FillMark::Check, [100.0, 620.0]);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "fill-sign", &app.application);
                return;
            };
            view.place_date(view.current(), [100.0, 570.0]);
        });
        let placed = Rc::new(std::cell::Cell::new(false));
        let app = self.clone();
        let placed_slot = placed.clone();
        glib::timeout_add_local_once(Duration::from_millis(3100), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "fill-sign", &app.application);
                return;
            };
            crate::app::native::save_saved_signature(false, "QA Firmado");
            let ok = view.place_saved_signature(view.current(), [100.0, 520.0], false);
            placed_slot.set(ok);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(3900), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "fill-sign", &app.application);
                return;
            };
            let annotations = view.info.borrow().annotations.clone();
            let total = annotations.len();
            let dates = annotations.iter().filter(|annotation| annotation.contents.as_deref().is_some_and(|contents| contents.contains('/'))).count();
            let fill_ok = placed.get() && total == before.get() + 3 && dates == 1;
            eprintln!("[DEBUG-fill] before={} total={total} dates={dates} placed={}", before.get(), placed.get());
            capture::window_png_when_ready(app.window.clone(), &capture::path("fill-sign"), 6, move |captured| {
                eprintln!("[DEBUG-fill] capture={captured:?}");
                capture::mark(fill_ok && captured.is_ok(), "fill-sign", &app.application);
            });
        });
    }

    fn schedule_sign_qa(self: &Rc<Self>) {
        let id_path = std::env::var("PDFCRAFT_GTK_QA_ID").unwrap_or_else(|_| "crates/sign/tests/data/ec-p256.p12".to_string());
        let app = self.clone();
        let id_fixture = id_path.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let created = app.automation.borrow().session().create_from_text("Firma QA", "Firmame");
            match created {
                Ok(bytes) => app.open_created("sign-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("sign: fixture fallo: {error}"),
            }
            crate::app::native::save_sign_id(&id_fixture);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "sign", &app.application);
                return;
            };
            view.open_sign_dialog(0, Some([20.0, 600.0, 220.0, 660.0]), None);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2300), move || {
            let Some(dialog) = signing::take_dialog() else {
                capture::mark(false, "sign", &app.application);
                return;
            };
            let path = capture::path("sign-dialog");
            match capture::window_png(&dialog, &path) {
                Ok(()) => eprintln!("[DEBUG-sign] dialog capturado"),
                Err(error) => eprintln!("[DEBUG-sign] dialog captura: {error}"),
            }
            dialog.close();
        });
        let performed = Rc::new(std::cell::Cell::new(false));
        let out_path = capture::artifact("sign-output.pdf");
        let app = self.clone();
        let performed_slot = performed.clone();
        let out_clone = out_path.clone();
        let id_source = id_path.clone();
        glib::timeout_add_local_once(Duration::from_millis(3100), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "sign", &app.application);
                return;
            };
            let result = signing::perform_sign(
                &app.automation,
                &view,
                &signing::SignRequest {
                    page: 0,
                    rect: Some([20.0, 600.0, 220.0, 660.0]),
                    certify: None,
                    id_path: id_source,
                    password: "test".to_string(),
                    reason: "QA".to_string(),
                    location: "Madrid".to_string(),
                },
                &out_clone,
            );
            performed_slot.set(result.is_ok());
            if let Err(error) = result {
                eprintln!("[DEBUG-sign] perform: {error}");
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "sign", &app.application);
                return;
            };
            let summary = {
                let automation = app.automation.borrow();
                automation.session().get(view.id).map(|doc| {
                    let visible = doc.signatures.first().is_some_and(|s| s.visible);
                    (
                        doc.is_signed(),
                        doc.signatures.len(),
                        doc.signatures.iter().filter(|s| matches!(s.status, SignatureStatus::Valid)).count(),
                        visible,
                    )
                })
            };
            let (is_signed, total, valid, visible) = summary.unwrap_or((false, 0, 0, false));
            let file_ok = std::fs::metadata(&out_path).map(|meta| meta.len() > 0).unwrap_or(false);
            let ok = performed.get() && is_signed && total == 1 && valid == 1 && visible && file_ok;
            eprintln!(
                "[DEBUG-sign] performed={} signed={is_signed} total={total} valid={valid} visible={visible} file_ok={file_ok}",
                performed.get()
            );
            capture::window_png_when_ready(app.window.clone(), &capture::path("sign-applied"), 6, move |captured| {
                capture::mark(ok && captured.is_ok(), "sign", &app.application);
            });
        });
    }

    fn schedule_page_ops_qa(self: &Rc<Self>) {
        fn json_content(contents: Vec<pdfcraft_automation::Content>) -> Option<serde_json::Value> {
            contents.into_iter().find_map(|content| match content {
                pdfcraft_automation::Content::Json(value) => Some(value),
                pdfcraft_automation::Content::Png { .. } => None,
            })
        }
        let original_size = Rc::new(RefCell::new(None::<(f32, f32)>));
        let numbered = Rc::new(RefCell::new(None::<serde_json::Value>));
        let cropped = Rc::new(RefCell::new(None::<serde_json::Value>));
        let replaced = Rc::new(RefCell::new(None::<serde_json::Value>));
        let split = Rc::new(RefCell::new(None::<serde_json::Value>));
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let combined = {
                let automation = app.automation.borrow();
                let session = automation.session();
                let make = |title: &str, text: &str| session.create_from_text(title, text);
                match (make("A", "A"), make("B", "B")) {
                    (Ok(a), Ok(b)) => session.combine(&[("A.pdf".to_string(), a), ("B.pdf".to_string(), b)]),
                    results => {
                        eprintln!("page-ops: fixture create_from_text fallo");
                        for result in [results.0, results.1] {
                            if let Err(error) = result {
                                eprintln!("page-ops: {error}");
                            }
                        }
                        return;
                    }
                }
            };
            match combined {
                Ok(bytes) => app.open_created("page-ops.pdf".to_string(), bytes),
                Err(error) => eprintln!("page-ops: combine fallo: {error}"),
            }
        });
        let app = self.clone();
        let size_slot = original_size.clone();
        let number_slot = numbered.clone();
        glib::timeout_add_local_once(Duration::from_millis(1300), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "page-ops", &app.application);
                return;
            };
            let size = {
                let automation = app.automation.borrow();
                automation.session().get(view.id).and_then(|doc| doc.info.pages.first().map(|page| (page.width, page.height)))
            };
            *size_slot.borrow_mut() = size;
            let result = app.automation.borrow_mut().call(
                "page_number",
                &serde_json::json!({ "doc": view.id.0, "from": 1, "to": 2, "style": "lower-roman", "prefix": "p-", "start": 3 }),
            );
            match result {
                Ok(contents) => *number_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("page-ops: page_number fallo: {error}"),
            }
        });
        let app = self.clone();
        let crop_slot = cropped.clone();
        glib::timeout_add_local_once(Duration::from_millis(1900), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "page-ops", &app.application);
                return;
            };
            let result = app
                .automation
                .borrow_mut()
                .call("page_set_box", &serde_json::json!({ "doc": view.id.0, "pages": [1], "box": "crop", "margins": [10.0, 20.0, 30.0, 40.0] }));
            match result {
                Ok(contents) => *crop_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("page-ops: page_set_box fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2400), move || {
            let created = app.automation.borrow().session().create_from_text("Reemplazo", "R");
            match created {
                Ok(bytes) => {
                    if let Err(error) = std::fs::write(capture::artifact("reemplazo.pdf"), bytes.as_ref()) {
                        eprintln!("page-ops: escribir reemplazo fallo: {error}");
                    }
                }
                Err(error) => eprintln!("page-ops: fixture reemplazo fallo: {error}"),
            }
        });
        let app = self.clone();
        let replace_slot = replaced.clone();
        glib::timeout_add_local_once(Duration::from_millis(2800), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "page-ops", &app.application);
                return;
            };
            let source = capture::artifact("reemplazo.pdf");
            let result = app
                .automation
                .borrow_mut()
                .call("page_replace", &serde_json::json!({ "doc": view.id.0, "pages": [2], "path": source.display().to_string() }));
            match result {
                Ok(contents) => *replace_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("page-ops: page_replace fallo: {error}"),
            }
        });
        let app = self.clone();
        let split_slot = split.clone();
        glib::timeout_add_local_once(Duration::from_millis(3300), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "page-ops", &app.application);
                return;
            };
            let out_dir = capture::artifact("parts");
            let _ = std::fs::create_dir_all(&out_dir);
            let result = app
                .automation
                .borrow_mut()
                .call("doc_split", &serde_json::json!({ "doc": view.id.0, "every": 1, "out_dir": out_dir.display().to_string() }));
            match result {
                Ok(contents) => *split_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("page-ops: doc_split fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4200), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "page-ops", &app.application);
                return;
            };
            let labels_ok = {
                let number_value = numbered.borrow().clone();
                let result_labels = number_value.as_ref().and_then(|value| value.get("labels").cloned());
                app.automation.borrow().session().get(view.id).is_some_and(|doc| {
                    let labels: Vec<String> = doc.info.pages.iter().map(|page| page.label.clone()).collect();
                    let ok = labels.len() == 2 && labels[0] == "p-iii" && labels[1] == "p-iv";
                    if !ok {
                        eprintln!("page-ops: labels={labels:?} result={result_labels:?}");
                    }
                    ok
                })
            };
            let crop_ok = {
                let crop_value = cropped.borrow().clone();
                let size = original_size.borrow();
                let sizes = crop_value.as_ref().and_then(|value| value.get("page_sizes").and_then(|sizes| sizes.get(0)).cloned());
                match (sizes, size.as_ref().copied()) {
                    (Some(first), Some((width, height))) => {
                        let expect_w = f64::from(width) - 40.0;
                        let expect_h = f64::from(height) - 60.0;
                        let got_w = first.get(0).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                        let got_h = first.get(1).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                        (got_w - expect_w).abs() < 0.5 && (got_h - expect_h).abs() < 0.5
                    }
                    _ => {
                        eprintln!("page-ops: crop sin page_sizes original={:?}", size.as_ref().copied());
                        false
                    }
                }
            };
            let replace_ok = {
                let replaced_value = replaced.borrow();
                replaced_value.is_some()
                    && app.automation.borrow().session().get(view.id).is_some_and(|doc| {
                        let text = doc.text_lines(1).iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join(" ");
                        let ok = view.page_count() == 2 && text.contains('R') && !text.contains('B');
                        if !ok {
                            eprintln!("page-ops: replace text={text:?}");
                        }
                        ok
                    })
            };
            let split_ok = {
                let split_value = split.borrow().clone();
                match split_value.as_ref().and_then(|value| value.get("files").and_then(|files| files.as_array()).cloned()) {
                    Some(files) => {
                        let count_ok = files.len() == 2;
                        let exist_ok = files.iter().all(|file| {
                            file.get("path")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|path| std::fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false))
                        });
                        count_ok && exist_ok
                    }
                    None => false,
                }
            };
            let ok = labels_ok && crop_ok && replace_ok && split_ok;
            eprintln!("[DEBUG-pageops] labels={labels_ok} crop={crop_ok} replace={replace_ok} split={split_ok}");
            capture::window_png_when_ready(app.window.clone(), &capture::path("page-ops"), 6, move |captured| {
                capture::mark(ok && captured.is_ok(), "page-ops", &app.application);
            });
        });
    }

    fn schedule_export_ops_qa(self: &Rc<Self>) {
        fn json_content(contents: Vec<pdfcraft_automation::Content>) -> Option<serde_json::Value> {
            contents.into_iter().find_map(|content| match content {
                pdfcraft_automation::Content::Json(value) => Some(value),
                pdfcraft_automation::Content::Png { .. } => None,
            })
        }
        let images = Rc::new(RefCell::new(None::<serde_json::Value>));
        let text = Rc::new(RefCell::new(None::<serde_json::Value>));
        let office = Rc::new(RefCell::new(None::<serde_json::Value>));
        let all_images = Rc::new(RefCell::new(None::<serde_json::Value>));
        let made_doc = Rc::new(Cell::new(0u64));
        let made_result = Rc::new(RefCell::new(None::<serde_json::Value>));
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let created = app.automation.borrow().session().create_from_text("Exportación", "Texto para export");
            match created {
                Ok(bytes) => app.open_created("export-ops.pdf".to_string(), bytes),
                Err(error) => eprintln!("export-ops: fixture fallo: {error}"),
            }
        });
        let app = self.clone();
        let images_slot = images.clone();
        glib::timeout_add_local_once(Duration::from_millis(1300), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "export-ops", &app.application);
                return;
            };
            let folder = capture::artifact("export-imgs");
            let _ = std::fs::create_dir_all(&folder);
            let result = app.automation.borrow_mut().call(
                "doc_export_images",
                &serde_json::json!({ "doc": view.id.0, "folder": folder.display().to_string(), "format": "png", "dpi": 72.0 }),
            );
            match result {
                Ok(contents) => *images_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("export-ops: doc_export_images fallo: {error}"),
            }
        });
        let app = self.clone();
        let text_slot = text.clone();
        glib::timeout_add_local_once(Duration::from_millis(1900), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "export-ops", &app.application);
                return;
            };
            let path = capture::artifact("export-ops.txt");
            let result =
                app.automation.borrow_mut().call("doc_export_text", &serde_json::json!({ "doc": view.id.0, "path": path.display().to_string() }));
            match result {
                Ok(contents) => *text_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("export-ops: doc_export_text fallo: {error}"),
            }
        });
        let app = self.clone();
        let office_slot = office.clone();
        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "export-ops", &app.application);
                return;
            };
            let path = capture::artifact("export-ops.docx");
            let result =
                app.automation.borrow_mut().call("doc_export_office", &serde_json::json!({ "doc": view.id.0, "path": path.display().to_string() }));
            match result {
                Ok(contents) => *office_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("export-ops: doc_export_office fallo: {error}"),
            }
        });
        let app = self.clone();
        let all_slot = all_images.clone();
        glib::timeout_add_local_once(Duration::from_millis(3100), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "export-ops", &app.application);
                return;
            };
            let folder = capture::artifact("export-imgs-all");
            let _ = std::fs::create_dir_all(&folder);
            let result = app
                .automation
                .borrow_mut()
                .call("doc_export_all_images", &serde_json::json!({ "doc": view.id.0, "folder": folder.display().to_string() }));
            match result {
                Ok(contents) => *all_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("export-ops: doc_export_all_images fallo: {error}"),
            }
        });
        let app = self.clone();
        let made_doc_slot = made_doc.clone();
        glib::timeout_add_local_once(Duration::from_millis(3500), move || {
            let source = capture::artifact("export-imgs/export-ops_page_1.png");
            let result =
                app.automation.borrow_mut().call("doc_create", &serde_json::json!({ "from": "images", "paths": [source.display().to_string()] }));
            match result {
                Ok(contents) => {
                    let value = json_content(contents);
                    if let Some(doc) = value.as_ref().and_then(|value| value.get("doc")).and_then(serde_json::Value::as_u64) {
                        made_doc_slot.set(doc);
                    }
                }
                Err(error) => eprintln!("export-ops: doc_create fallo: {error}"),
            }
        });
        let app = self.clone();
        let made_slot = made_result.clone();
        glib::timeout_add_local_once(Duration::from_millis(3900), move || {
            let doc = made_doc.get();
            if doc == 0 {
                eprintln!("export-ops: doc_create no devolvio doc");
                return;
            }
            let folder = capture::artifact("export-imgs-made");
            let _ = std::fs::create_dir_all(&folder);
            let result =
                app.automation.borrow_mut().call("doc_export_all_images", &serde_json::json!({ "doc": doc, "folder": folder.display().to_string() }));
            match result {
                Ok(contents) => *made_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("export-ops: all_images(made) fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4300), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "export-ops", &app.application);
                return;
            };
            let _ = view;
            let images_value = images.borrow().clone();
            let images_ok = {
                let ok = images_value.as_ref().and_then(|value| value.get("count").and_then(serde_json::Value::as_u64)) == Some(1);
                let file_ok = images_value
                    .as_ref()
                    .and_then(|value| value.get("files").and_then(|files| files.get(0)).cloned())
                    .and_then(|file| file.as_str().map(ToOwned::to_owned))
                    .is_some_and(|path| {
                        std::fs::read(&path).map(|bytes| bytes.len() > 100 && bytes.starts_with(&[0x89, b'P', b'N', b'G'])).unwrap_or(false)
                    });
                ok && file_ok
            };
            let text_value = text.borrow().clone();
            let text_ok = text_value.as_ref().and_then(|value| value.get("bytes").and_then(serde_json::Value::as_u64)).is_some_and(|bytes| bytes > 0)
                && std::fs::read_to_string(capture::artifact("export-ops.txt")).map(|content| content.contains("Texto para export")).unwrap_or(false);
            let office_value = office.borrow().clone();
            let office_ok = {
                let format_ok = office_value.as_ref().and_then(|value| value.get("format").and_then(serde_json::Value::as_str)) == Some("docx");
                let bytes_ok =
                    office_value.as_ref().and_then(|value| value.get("bytes").and_then(serde_json::Value::as_u64)).is_some_and(|bytes| bytes > 100)
                        && std::fs::read(capture::artifact("export-ops.docx")).map(|bytes| bytes.starts_with(b"PK")).unwrap_or(false);
                format_ok && bytes_ok
            };
            let all_value = all_images.borrow().clone();
            let all_ok = all_value.as_ref().and_then(|value| value.get("count").and_then(serde_json::Value::as_u64)) == Some(0);
            let made_value = made_result.borrow().clone();
            let made_ok = {
                let count_ok = made_value.as_ref().and_then(|value| value.get("count").and_then(serde_json::Value::as_u64)) == Some(1);
                let file_ok = made_value
                    .as_ref()
                    .and_then(|value| value.get("files").and_then(|files| files.get(0)).cloned())
                    .and_then(|file| file.get("path").and_then(serde_json::Value::as_str).map(str::to_owned))
                    .is_some_and(|path| {
                        path.ends_with("_Page_1_Image_0001.png")
                            && std::fs::read(&path).map(|bytes| bytes.starts_with(&[0x89, b'P', b'N', b'G'])).unwrap_or(false)
                    });
                count_ok && file_ok
            };
            let ok = images_ok && text_ok && office_ok && all_ok && made_ok;
            eprintln!("[DEBUG-exportops] images={images_ok} text={text_ok} office={office_ok} all_images={all_ok} made={made_ok}");
            capture::window_png_when_ready(app.window.clone(), &capture::path("export-ops"), 6, move |captured| {
                capture::mark(ok && captured.is_ok(), "export-ops", &app.application);
            });
        });
    }

    fn schedule_parity_ops_qa(self: &Rc<Self>) {
        fn json_content(contents: Vec<pdfcraft_automation::Content>) -> Option<serde_json::Value> {
            contents.into_iter().find_map(|content| match content {
                pdfcraft_automation::Content::Json(value) => Some(value),
                pdfcraft_automation::Content::Png { .. } => None,
            })
        }
        let other_id = Rc::new(Cell::new(0u64));
        let compare_json = Rc::new(RefCell::new(None::<serde_json::Value>));
        let tag_json = Rc::new(RefCell::new(None::<serde_json::Value>));
        let restored = Rc::new(Cell::new(false));
        let fixture = capture::artifact("parity-fixture.pdf");
        let app = self.clone();
        let other_slot = other_id.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            // El fixture lo abre el opener genérico de schedule_qa (QA_FILE):
            // aquí solo creamos el segundo documento para comparar.
            let result = app
                .automation
                .borrow_mut()
                .call("doc_create", &serde_json::json!({ "from": "text", "text": "Hola mundo cambiado", "name": "parity-other.pdf" }));
            match result {
                Ok(contents) => {
                    if let Some(value) = json_content(contents) {
                        let id = value.get("doc").and_then(serde_json::Value::as_u64).unwrap_or(0);
                        other_slot.set(id);
                    }
                }
                Err(error) => eprintln!("parity-ops: doc_create fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1300), move || {
            app.close_active();
            eprintln!("[DEBUG-parityops] tras cerrar views={} history={}", app.views.borrow().len(), app.closed_tabs.borrow().len());
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2000), move || {
            app.reopen_closed_tab();
            eprintln!("[DEBUG-parityops] tras reabrir views={}", app.views.borrow().len());
        });
        let app = self.clone();
        let compare_slot = compare_json.clone();
        let restored_slot = restored.clone();
        let other_for_compare = other_id.clone();
        glib::timeout_add_local_once(Duration::from_millis(3000), move || {
            let path_restored = app.active_view().and_then(|view| view.path()).is_some_and(|path| path == fixture);
            let views_ok = app.views.borrow().len() == 1;
            restored_slot.set(views_ok && path_restored);
            eprintln!("[DEBUG-parityops] views={} path_restored={path_restored}", app.views.borrow().len());
            if !restored_slot.get() {
                capture::mark(false, "parity-ops", &app.application);
                return;
            }
            let Some(view) = app.active_view() else {
                capture::mark(false, "parity-ops", &app.application);
                return;
            };
            let other = other_for_compare.get();
            if other == 0 {
                eprintln!("parity-ops: doc_create no devolvio id");
                capture::mark(false, "parity-ops", &app.application);
                return;
            }
            let result = app.automation.borrow_mut().call("doc_compare", &serde_json::json!({ "doc": view.id.0, "other": other }));
            match result {
                Ok(contents) => *compare_slot.borrow_mut() = json_content(contents),
                Err(error) => eprintln!("parity-ops: doc_compare fallo: {error}"),
            }
        });
        let app = self.clone();
        let tag_slot = tag_json.clone();
        glib::timeout_add_local_once(Duration::from_millis(3600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "parity-ops", &app.application);
                return;
            };
            // Autotag de texto; si el documento tiene campos de formulario el
            // motor lo rechaza y se usa el autotag de campos (a11y.autotag_fields).
            let result = app.automation.borrow_mut().call("accessibility_autotag_apply", &serde_json::json!({ "doc": view.id.0 }));
            let result = match result {
                Ok(contents) => Ok(contents),
                Err(text_error) => {
                    eprintln!("parity-ops: autotag texto no aplica ({text_error}); probando autotag de campos");
                    app.automation.borrow_mut().call("accessibility_autotag_form_fields", &serde_json::json!({ "doc": view.id.0 }))
                }
            };
            match result {
                Ok(contents) => {
                    *tag_slot.borrow_mut() = json_content(contents);
                    view.refresh(&app.automation);
                }
                Err(error) => eprintln!("parity-ops: autotag fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4400), move || {
            let compare_value = compare_json.borrow().clone();
            let changes = |value: &serde_json::Value, key: &str| value.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
            let compare_ok = compare_value.as_ref().is_some_and(|value| {
                value.get("identical").and_then(serde_json::Value::as_bool) == Some(false)
                    && changes(value, "replaced") + changes(value, "inserted") + changes(value, "deleted") > 0
            });
            let tag_value = tag_json.borrow().clone();
            let tag_ok = tag_value.as_ref().is_some_and(|value| {
                value.get("count").and_then(serde_json::Value::as_u64).is_some_and(|count| count > 0)
                    || (value.get("status").and_then(serde_json::Value::as_str) == Some("pass")
                        && value.get("remaining_findings").and_then(serde_json::Value::as_u64) == Some(0))
            });
            let tag_id = tag_value.as_ref().and_then(|value| value.get("status").and_then(serde_json::Value::as_str)).unwrap_or("-");
            let ok = restored.get() && compare_ok && tag_ok;
            eprintln!("[DEBUG-parityops] reopen={} compare={compare_ok} tag={tag_ok} tag_status={tag_id}", restored.get());
            capture::window_png_when_ready(app.window.clone(), &capture::path("parity-ops"), 6, move |captured| {
                capture::mark(ok && captured.is_ok(), "parity-ops", &app.application);
            });
        });
    }

    fn schedule_organizer_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let combined = {
                let automation = app.automation.borrow();
                let session = automation.session();
                let make = |title: &str, text: &str| session.create_from_text(title, text);
                match (make("A", "A"), make("B", "B"), make("C", "C")) {
                    (Ok(a), Ok(b), Ok(c)) => session.combine(&[("A.pdf".to_string(), a), ("B.pdf".to_string(), b), ("C.pdf".to_string(), c)]),
                    results => {
                        eprintln!("organizer: fixture create_from_text fallo");
                        for result in [results.0, results.1, results.2] {
                            if let Err(error) = result {
                                eprintln!("organizer: {error}");
                            }
                        }
                        return;
                    }
                }
            };
            match combined {
                Ok(bytes) => app.open_created("organizer-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("organizer: combine fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1400), move || {
            app.run_editing("page.organize");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1800), move || {
            // Mismo camino que los botones del organizador: bajar A, rotar C, borrar B.
            app.apply_edit(Edit::MovePages { pages: vec![0], to: 1 });
            app.apply_edit(Edit::RotatePages { pages: vec![2], degrees: 90 });
            app.apply_edit(Edit::DeletePages { pages: vec![0] });
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2600), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "organizer", &app.application);
                return;
            };
            let ok = app
                .automation
                .borrow()
                .session()
                .get(view.id)
                .map(|document| {
                    let page_text = |page: usize| document.text_lines(page).iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join(" ");
                    let first = page_text(0);
                    let second = page_text(1);
                    let rotation = document.info.pages.get(1).map_or(0, |page| page.rotation);
                    eprintln!("organizer pages={} first={first:?} second={second:?} rotation_p2={rotation}", view.page_count());
                    view.page_count() == 2 && first.contains('A') && !first.contains('B') && second.contains('C') && rotation == 90
                })
                .unwrap_or(false);
            let path = capture::path("organizer-ops");
            eprintln!("organizer ops_ok={ok}");
            capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                eprintln!("organizer captured={}", captured.is_ok());
                capture::mark(ok && captured.is_ok(), "organizer", &app.application);
            });
        });
    }

    fn schedule_panels_edit_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let fixture = {
                let automation = app.automation.borrow();
                let session = automation.session();
                session.create_from_text("Paneles QA", "Panel")
            };
            match fixture {
                Ok(bytes) => app.open_created("panels-fixture.pdf".to_string(), bytes),
                Err(error) => eprintln!("panels: fixture fallo: {error}"),
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1100), move || {
            // Semilla: 2 marcadores (raíz + hijo), 1 adjunto y 1 nota, por Edits reales.
            app.apply_edit(Edit::AddBookmark { parent: vec![], index: 0, title: "Raíz".to_string(), page: 0, left: None, top: None });
            app.apply_edit(Edit::AddBookmark { parent: vec![0], index: 0, title: "Hijo".to_string(), page: 0, left: None, top: None });
            app.apply_edit(Edit::AddAttachment {
                name: "nota.txt".to_string(),
                bytes: b"hola".to_vec(),
                description: Some("prueba".to_string()),
                mime: Some("text/plain".to_string()),
            });
            app.apply_edit(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page: 0,
                shape: pdfcraft_engine::Shape::Note { at: [72.0, 700.0], icon: pdfcraft_engine::NoteIcon::Comment },
                style: pdfcraft_engine::Style::default(),
                contents: "nota".to_string(),
                author: "panels".to_string(),
            }));
            app.panels.select_section("Marcadores");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2100), move || {
            // Camino del botón + del panel: añadir marcador raíz.
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            app.panels.add_bookmark(&view);
            app.run_native("view.side_panel");
            app.panels.select_section("Marcadores");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            app.panels.rename_bookmark(&view, vec![1], "Nuevo");
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2900), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            app.panels.delete_bookmark(&view, vec![0, 0]);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(3300), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let result = app.automation.borrow_mut().call(
                "form_add_field",
                &serde_json::json!({"doc": view.id.0, "page": 1, "type": "text", "rect": [60.0, 600.0, 200.0, 630.0], "name": "CampoQA"}),
            );
            if let Err(error) = result {
                eprintln!("panels: form_add_field fallo: {error}");
                capture::mark(false, "panels-edit", &app.application);
            }
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(3700), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let Some(field) = app.automation.borrow().session().get(view.id).and_then(|document| document.info.fields.first().cloned()) else {
                eprintln!("panels: sin campos en fixture");
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let value = match field.kind {
                FieldInfoKind::Text => FieldValue::Text("QA".to_string()),
                FieldInfoKind::CheckBox => FieldValue::Check(!matches!(field.value.as_deref(), Some("Yes") | Some("On") | Some("1"))),
                _ => {
                    eprintln!("panels: campo {:?} no editable en humo", field.kind);
                    capture::mark(false, "panels-edit", &app.application);
                    return;
                }
            };
            app.panels.set_field_value(&view, &field.name, value);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4100), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let key = app.automation.borrow().session().get(view.id).and_then(|document| document.info.attachments.first().cloned()).and_then(
                |attachment| match attachment.source {
                    AttachmentSource::Document { key } => Some(key),
                    _ => None,
                },
            );
            let Some(key) = key else {
                eprintln!("panels: sin adjunto de documento");
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            app.panels.delete_attachment(&view, key);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let index = app
                .automation
                .borrow()
                .session()
                .get(view.id)
                .and_then(|document| document.info.annotations.first().map(|annotation| annotation.index));
            let Some(index) = index else {
                eprintln!("panels: sin anotaciones");
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            app.panels.delete_comment(&view, 0, index);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(5000), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "panels-edit", &app.application);
                return;
            };
            let (ok, roots, children, field_value, attachments, comments, layers) = {
                let automation = app.automation.borrow();
                let Some(document) = automation.session().get(view.id) else {
                    capture::mark(false, "panels-edit", &app.application);
                    return;
                };
                let titles = |items: &[pdfcraft_engine::OutlineItem]| items.iter().map(|item| item.title.clone()).collect::<Vec<_>>();
                let roots = titles(&document.info.outline);
                let children = document.info.outline.first().map(|item| titles(&item.children)).unwrap_or_default();
                let field_value = document.info.fields.first().and_then(|field| field.value.clone());
                let attachments = document.info.attachments.len();
                let comments = document.info.annotations.len();
                let layers = document.info.layers.len();
                let ok =
                    roots == ["Raíz", "Nuevo"] && children.is_empty() && field_value.as_deref() == Some("QA") && attachments == 0 && comments == 0;
                (ok, roots, children, field_value, attachments, comments, layers)
            };
            eprintln!(
                "panels-edit roots={roots:?} children={children:?} field={field_value:?} attachments={attachments} comments={comments} layers={layers}"
            );
            // Capas: la fixture no tiene capas; validamos el reset no-panic vía API (sin borrow pendiente).
            let _layers_changed = app.automation.borrow_mut().session_mut().reset_layers(view.id);
            let _ = _layers_changed;
            let path = capture::path("panels-edit");
            eprintln!("panels-edit ops_ok={ok}");
            capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                eprintln!("panels-edit captured={}", captured.is_ok());
                capture::mark(ok && captured.is_ok(), "panels-edit", &app.application);
            });
        });
    }

    fn schedule_coverage_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let mut native = 0usize;
            let mut automated = 0usize;
            let mut missing = Vec::new();
            for command in commands::COMMANDS {
                if has_native_handler(command.id) {
                    native += 1;
                } else if automation::supports_command(command.id) {
                    automated += 1;
                } else {
                    missing.push(command.id);
                }
            }
            let image = capture::path("coverage");
            eprintln!("coverage total={} native={native} automated={automated} missing={}", commands::COMMANDS.len(), missing.len());
            if !missing.is_empty() {
                eprintln!("coverage missing commands: {}", missing.join(", "));
            }
            capture::window_png_when_ready(app.window.clone(), &image, 8, move |captured| {
                capture::mark(missing.is_empty() && captured.is_ok(), "coverage", &app.application);
            });
        });
    }

    fn schedule_reconcile_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1800), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "reconcile", &app.application);
                return;
            };
            let output = capture::artifact("reconcile-output.pdf");
            let args = serde_json::json!({"id": "page.extract", "params": {"doc": view.id.0, "pages": [1], "open": true, "out": output}});
            let result = app.automation.borrow_mut().call("command_run", &args);
            if let Err(error) = result {
                eprintln!("reconcile command failed: {error}");
                capture::mark(false, "reconcile", &app.application);
                return;
            }
            app.reconcile_documents();
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4000), move || {
            let documents = app.automation.borrow().session().docs().len();
            let views = app.views.borrow().len();
            let tabs = app.tabs.n_pages();
            let image = capture::path("reconcile");
            eprintln!("reconcile documents={documents} views={views} tabs={tabs}");
            capture::window_png_when_ready(app.window.clone(), &image, 8, move |captured| {
                eprintln!("reconcile capture={}", captured.is_ok());
                capture::mark(documents == 2 && views == 2 && tabs == 2 && captured.is_ok(), "reconcile", &app.application);
            });
        });
    }

    fn schedule_viewer_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "viewer", &app.application);
                return;
            };
            view.next(&app.automation);
            view.zoom_in(&app.automation);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(4500), move || {
            let Some(view) = app.active_view() else {
                capture::mark(false, "viewer", &app.application);
                return;
            };
            let functional_ok = view.has_page() && view.current() == 1 && view.zoom_label.text() == "125 %";
            if !functional_ok {
                capture::mark(false, "viewer", &app.application);
                return;
            }
            let automation = app.automation.clone();
            let path = capture::path("viewer");
            capture::window_png_when_ready(app.window.clone(), &path, 8, move |captured| {
                if !captured.is_ok() {
                    capture::mark(false, "viewer", &app.application);
                    return;
                }
                if let Some(view) = app.active_view() {
                    view.zoom_out(&automation);
                    view.previous(&automation);
                }
                app.close_active();
                let ok = app.views.borrow().is_empty() && app.tabs.n_pages() == 0;
                capture::mark(ok, "viewer", &app.application);
            });
        });
    }
}

fn has_native_handler(id: &str) -> bool {
    matches!(
        id,
        "file.open"
            | "create.blank"
            | "file.save"
            | "file.save_as"
            | "file.revert"
            | "file.close"
            | "file.close_all"
            | "edit.undo"
            | "edit.redo"
            | "page.rotate"
            | "page.rotate_ccw"
            | "page.delete"
            | "page.insert_blank"
            | "page.duplicate"
            | "page.reverse"
            | "view.palette"
            | "view.next_tab"
            | "view.previous_tab"
            | "view.full_screen"
            | "help.app_page"
            | "help.github"
            | "help.about"
    ) || App::supports_native(id)
        || App::supports_editing(id)
        || App::supports_signing(id)
}

fn side_panel(title: &str, description: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let label = gtk::Label::new(Some(description));
    label.set_wrap(true);
    label.set_margin_top(10);
    label.set_margin_bottom(4);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_css_classes(&["dim-label", "caption"]);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&label);
    body.append(child);
    let frame = gtk::Frame::new(Some(title));
    frame.set_child(Some(&body));
    frame
}

fn command_action(id: &str) -> String {
    format!("cmd-{}", id.replace('.', "-"))
}

fn command_item(command: &commands::CommandSpec) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(command.label), Some(&format!("win.{}", command_action(command.id))));
    if let Some(shortcut) = command.shortcut {
        let accelerator = command_accelerator(shortcut).to_variant();
        item.set_attribute_value("accel", Some(&accelerator));
    }
    item
}

fn command_accelerator(shortcut: commands::Shortcut) -> String {
    let mut accelerator = String::new();
    if shortcut.command || shortcut.mac_ctrl {
        accelerator.push_str("<Ctrl>");
    }
    if shortcut.shift {
        accelerator.push_str("<Shift>");
    }
    let key = match shortcut.key {
        "[" => "bracketleft".to_string(),
        "]" => "bracketright".to_string(),
        "," => "comma".to_string(),
        "." => "period".to_string(),
        "-" => "minus".to_string(),
        "+" => "plus".to_string(),
        "=" => "equal".to_string(),
        ";" => "semicolon".to_string(),
        "'" => "apostrophe".to_string(),
        "/" => "slash".to_string(),
        "\\" => "backslash".to_string(),
        " " => "space".to_string(),
        key if key.chars().count() == 1 => key.to_lowercase(),
        key => key.to_string(),
    };
    accelerator.push_str(&key);
    accelerator
}
